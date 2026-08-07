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

fn main() {
    // Recompute whenever the pinned version changes.
    println!("cargo:rerun-if-env-changed=WEVERYWHERE_VERSION");

    let version = std::env::var("WEVERYWHERE_VERSION")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(compute_version);

    println!("cargo:rustc-env=WEVERYWHERE_VERSION={version}");
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
