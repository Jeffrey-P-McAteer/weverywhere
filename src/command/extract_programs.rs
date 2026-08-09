
use super::*;

/// Write every embedded WASI program (see [`crate::embedded_programs`]) to `out_dir` as
/// `<name>.wasm`. This is the file-side counterpart to running the same programs from the in-memory
/// bytes; it lets a deployed, file-free binary still hand its bundled programs to other tools.
pub async fn extract_programs(out_dir: &std::path::PathBuf) -> DynResult<()> {
  let programs = crate::embedded_programs::all();

  if programs.is_empty() {
    println!("This binary has no embedded programs.");
    println!("(Build with zig installed and names listed in example-programs/embedded.list to embed some.)");
    return Ok(());
  }

  tokio::fs::create_dir_all(out_dir).await.map_err(map_loc_err!())?;

  for (name, bytes) in programs {
    let dest = out_dir.join(format!("{name}.wasm"));
    tokio::fs::write(&dest, bytes).await.map_err(map_loc_err!())?;
    println!("wrote {} ({})", dest.display(), fs_utils::format_size_bytes(bytes.len()));
  }

  println!("Extracted {} embedded program(s) to {}", programs.len(), out_dir.display());
  Ok(())
}
