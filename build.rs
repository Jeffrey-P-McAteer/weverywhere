// Bakes the build version into the binary.
//
// Version scheme (see scripts/_version.py, the single source of truth): YYYY.MM.H
// where H = whole hours elapsed since the start of the current month (UTC).
//
// The release pipeline (scripts/build.py / scripts/publish.py) resolves the
// version once and exports WEVERYWHERE_VERSION so the git tag, the GitHub asset
// names, and the value baked in here can never drift across an hour boundary.
// A plain `cargo build` with no env var set computes the same formula locally,
// so there is never a hard-coded version string to maintain.

use std::time::{SystemTime, UNIX_EPOCH};
use std::path::{Path, PathBuf};

fn main() {
    // Recompute whenever the pinned version changes.
    println!("cargo:rerun-if-env-changed=WEVERYWHERE_VERSION");

    let version = std::env::var("WEVERYWHERE_VERSION")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(compute_version);

    println!("cargo:rustc-env=WEVERYWHERE_VERSION={version}");

    embed_example_programs();
}

/// Compile the programs named in `example-programs/embedded.list` and generate
/// `$OUT_DIR/embedded_programs.rs`, an `EMBEDDED_PROGRAMS: &[(&str, &[u8])]` table that
/// `src/embedded_programs.rs` includes. This lets a deployed binary run bundled programs (e.g. the
/// `netmap` discovery program) with no external .wasm file.
///
/// Compilation uses zig, exactly like `scripts/compile-example-programs.py`. If zig is missing or a
/// program fails to build we emit a `cargo:warning` and simply omit that entry (an empty/partial
/// table), so a plain `cargo build` never hard-fails on account of embedding — callers just fall
/// back to `--program` / the on-disk example.
fn embed_example_programs() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let examples_dir = manifest_dir.join("example-programs");
    let list_path = examples_dir.join("embedded.list");

    // Rebuild the table if the list changes (adding/removing a program).
    println!("cargo:rerun-if-changed={}", list_path.display());

    let names = read_embed_list(&list_path);

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for name in &names {
        let source = examples_dir.join(format!("{name}.c"));
        // Rebuild whenever the source changes.
        println!("cargo:rerun-if-changed={}", source.display());

        if !source.exists() {
            println!("cargo:warning=embedded.list names '{name}' but {} does not exist; skipping", source.display());
            continue;
        }

        let out_wasm = out_dir.join(format!("{name}.wasm"));
        match compile_one(&source, &out_wasm) {
            Ok(()) => entries.push((name.clone(), out_wasm)),
            Err(e) => println!("cargo:warning=could not embed program '{name}': {e}. `netmap` will fall back to --program / the on-disk example."),
        }
    }

    write_generated_table(&out_dir, &entries);
}

/// Parse the embed manifest: one program stem per line, '#' comments and blanks ignored.
fn read_embed_list(list_path: &Path) -> Vec<String> {
    match std::fs::read_to_string(list_path) {
        Ok(contents) => contents
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect(),
        Err(_) => {
            // No manifest => nothing to embed. Not an error.
            Vec::new()
        }
    }
}

/// Compile one C example to `out_wasm` using zig, honouring an optional `// COMPILE:` line in the
/// source (mirrors scripts/compile-example-programs.py). Returns Err with a message on any failure.
fn compile_one(source: &Path, out_wasm: &Path) -> Result<(), String> {
    let argv = compile_command_for(source);
    // Template THIS_FILE / OUT_FILE, like the python script does.
    let argv: Vec<String> = argv.into_iter().map(|tok| match tok.as_str() {
        "THIS_FILE" => source.display().to_string(),
        "OUT_FILE" => out_wasm.display().to_string(),
        _ => tok,
    }).collect();

    let (program, rest) = argv.split_first().ok_or_else(|| "empty compile command".to_string())?;

    let output = std::process::Command::new(program)
        .args(rest)
        .output()
        .map_err(|e| format!("failed to run '{program}' (is it installed and on PATH?): {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "{} exited with {}: {}",
            program,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if !out_wasm.exists() {
        return Err(format!("{} reported success but {} was not produced", program, out_wasm.display()));
    }
    Ok(())
}

/// The default compile command, overridable by a `// COMPILE: ...` line in the source. Uses simple
/// whitespace splitting (the example commands contain no quoted spaces).
fn compile_command_for(source: &Path) -> Vec<String> {
    let default = "zig cc -target wasm32-wasi -O2 -o OUT_FILE THIS_FILE";
    let line = std::fs::read_to_string(source)
        .ok()
        .and_then(|src| {
            src.lines()
                .find_map(|l| l.trim_start().strip_prefix("// COMPILE:").map(|c| c.trim().to_string()))
        })
        .unwrap_or_else(|| default.to_string());
    line.split_whitespace().map(|s| s.to_string()).collect()
}

/// Emit `$OUT_DIR/embedded_programs.rs` with the `EMBEDDED_PROGRAMS` table. Uses absolute paths in
/// `include_bytes!` because this file is textually `include!`-ed into src/embedded_programs.rs, and
/// relative include paths would otherwise resolve against the wrong directory.
fn write_generated_table(out_dir: &Path, entries: &[(String, PathBuf)]) {
    let mut rust = String::new();
    rust.push_str("// @generated by build.rs from example-programs/embedded.list — do not edit.\n");
    rust.push_str("pub static EMBEDDED_PROGRAMS: &[(&str, &[u8])] = &[\n");
    for (name, path) in entries {
        // Rust string-escape the path (Windows backslashes etc.).
        // include_bytes! yields &[u8; N]; the &[u8] element type coerces it (unsize coercion is
        // allowed in statics, unlike the `[..]` index operator which is non-const).
        rust.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            name,
            path.display().to_string()
        ));
    }
    rust.push_str("];\n");

    let dest = out_dir.join("embedded_programs.rs");
    std::fs::write(&dest, rust).expect("failed to write generated embedded_programs.rs");
}

/// YYYY.MM.H, H = whole hours elapsed since the start of the month (UTC).
///
/// Implemented with a small civil-calendar conversion so the crate pulls in no
/// extra build dependency. Mirrors scripts/_version.py::compute_version.
fn compute_version() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let days = (secs / 86_400) as i64;
    let secs_of_day = secs % 86_400;
    let hour = (secs_of_day / 3_600) as u32;

    let (year, month, day) = civil_from_days(days);
    let hours_in_month = (day - 1) * 24 + hour;
    format!("{year}.{month:02}.{hours_in_month}")
}

/// Convert a count of days since the Unix epoch (1970-01-01) into a (year,
/// month, day) civil date (UTC). Algorithm from Howard Hinnant's `chrono`-style
/// civil calendar routines (public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}
