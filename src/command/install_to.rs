
use super::*;

/// We embed the folder ./etc for extraction when installing; this allows users without package managers who embed the files
/// to easily extract their own.
static TEMPLATE_ETC_DIR: include_directory::Dir<'_> = include_directory::include_directory!("$CARGO_MANIFEST_DIR/etc");

pub async fn install_to(install_root: &std::path::PathBuf, install_etc: &std::path::PathBuf, install_bin: &std::path::PathBuf) -> DynResult<()> {

  for dirent in TEMPLATE_ETC_DIR.entries() {
    tracing::warn!("TODO extract {:?} to {:?} / {:?}", dirent, install_root, install_etc);
  }

  tracing::warn!("This has not been implemented yet, see {}:{}", file!(), line!());

  Ok(())
}
