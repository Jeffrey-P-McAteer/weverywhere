
use super::*;

/// We embed the folder ./etc for extraction when installing; this allows users without package managers who embed the files
/// to easily extract their own.
static TEMPLATE_ETC_DIR: include_directory::Dir<'_> = include_directory::include_directory!("$CARGO_MANIFEST_DIR/etc");

/// Extract the embedded `etc/` templates under INSTALL_ROOT/INSTALL_ETC and copy this running
/// executable to INSTALL_ROOT/INSTALL_BIN/weverywhere[.exe]. This lays down everything a machine
/// needs to then register + run the daemon (see `weverywhere daemon --help`).
pub async fn install_to(install_root: &std::path::PathBuf, install_etc: &std::path::PathBuf, install_bin: &std::path::PathBuf) -> DynResult<()> {

  let etc_dest = install_root.join(install_etc);
  let bin_dest_dir = install_root.join(install_bin);

  // --- 1. Extract embedded etc/ templates --------------------------------------------------
  tokio::fs::create_dir_all(&etc_dest).await.map_err(map_loc_err!())?;
  let mut extracted = 0usize;
  for entry in TEMPLATE_ETC_DIR.entries() {
    extract_entry(entry, &etc_dest, &mut extracted).await.map_err(map_loc_err!())?;
  }
  tracing::info!("Extracted {} config file(s) to {}", extracted, etc_dest.display());

  // --- 2. Copy this executable into place --------------------------------------------------
  tokio::fs::create_dir_all(&bin_dest_dir).await.map_err(map_loc_err!())?;
  let this_exe = std::env::current_exe().map_err(map_loc_err!())?;
  let exe_file_name = this_exe.file_name()
    .map(|n| n.to_owned())
    .unwrap_or_else(|| std::ffi::OsString::from(default_binary_name()));
  let bin_dest = bin_dest_dir.join(&exe_file_name);

  // Avoid copying a file onto itself (e.g. re-running install-to from the installed location).
  if same_file(&this_exe, &bin_dest) {
    tracing::info!("Binary already installed at {}", bin_dest.display());
  }
  else {
    tokio::fs::copy(&this_exe, &bin_dest).await.map_err(map_loc_err!())?;
    ensure_executable(&bin_dest).await.map_err(map_loc_err!())?;
    tracing::info!("Installed binary to {}", bin_dest.display());
  }

  // --- 3. Tell the user how to bring the daemon up -----------------------------------------
  print_next_steps(&bin_dest, &etc_dest);

  Ok(())
}

/// Recursively write one embedded entry (file or directory) beneath `dest`.
async fn extract_entry(entry: &include_directory::DirEntry<'_>, dest: &std::path::Path, count: &mut usize) -> DynResult<()> {
  match entry {
    include_directory::DirEntry::Dir(dir) => {
      let name = leaf_name(dir.path());
      let sub_dest = dest.join(name);
      tokio::fs::create_dir_all(&sub_dest).await.map_err(map_loc_err!())?;
      // include_directory has no async recursion helper, so Box the recursive future.
      for child in dir.entries() {
        Box::pin(extract_entry(child, &sub_dest, count)).await.map_err(map_loc_err!())?;
      }
    }
    include_directory::DirEntry::File(file) => {
      let name = leaf_name(file.path());
      let file_dest = dest.join(name);
      tokio::fs::write(&file_dest, file.contents()).await.map_err(map_loc_err!())?;
      tracing::info!("  wrote {}", file_dest.display());
      *count += 1;
    }
  }
  Ok(())
}

/// The final path component (the embedded paths are relative to the etc/ root).
fn leaf_name(path: &std::path::Path) -> std::ffi::OsString {
  path.file_name().map(|n| n.to_owned()).unwrap_or_else(|| path.as_os_str().to_owned())
}

fn default_binary_name() -> &'static str {
  if cfg!(windows) { "weverywhere.exe" } else { "weverywhere" }
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
  match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
    (Ok(a), Ok(b)) => a == b,
    _ => false,
  }
}

/// Mark the freshly copied binary as executable on unix; a no-op on Windows.
async fn ensure_executable(path: &std::path::Path) -> DynResult<()> {
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = tokio::fs::metadata(path).await.map_err(map_loc_err!())?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    tokio::fs::set_permissions(path, perms).await.map_err(map_loc_err!())?;
  }
  #[cfg(not(unix))]
  {
    let _ = path; // nothing to do on Windows
  }
  Ok(())
}

fn print_next_steps(bin_dest: &std::path::Path, etc_dest: &std::path::Path) {
  let bin = bin_dest.display();
  println!("\nweverywhere installed:");
  println!("  binary:  {}", bin);
  println!("  config:  {}", etc_dest.display());
  println!("\nNext, register and start the background daemon (as root/Administrator):");
  #[cfg(target_os = "windows")]
  println!("  \"{}\" daemon install", bin);
  #[cfg(not(target_os = "windows"))]
  println!("  sudo \"{}\" daemon install", bin);
  println!("\nManage it later with: daemon status | start | stop | restart | uninstall");
}
