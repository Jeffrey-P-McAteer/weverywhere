
use crate::*;
use crate::args::*;

pub async fn run_command(cmd: &args::Command, args: &args::Args) -> DynResult<()> {

  match cmd {
    Command::Info { file_path } => {
      info(file_path).await?;
    }
    Command::InstallTo { install_root, install_etc, install_bin } => {
      install_to(install_root, install_etc, install_bin).await?;
    }
    Command::Run { file_path, multicast_group, port } => {
      run(file_path, multicast_group, *port).await?;
    }
    Command::Serve { multicast_group, port } => {
      serve(multicast_group, *port).await?;
    }
  }

  Ok(())
}

pub async fn info(file_path: &std::path::PathBuf) -> DynResult<()> {

  Ok(())
}

pub async fn install_to(install_root: &std::path::PathBuf, install_etc: &Option<std::path::PathBuf>, install_bin: &Option<std::path::PathBuf>) -> DynResult<()> {

  Ok(())
}

pub async fn run(file_path: &std::path::PathBuf, multicast_group: &core::net::IpAddr, port: u16) -> DynResult<()> {

  Ok(())
}

pub async fn serve(multicast_group: &core::net::IpAddr, port: u16) -> DynResult<()> {

  Ok(())
}




