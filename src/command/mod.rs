
use crate::*;
use crate::args::*;

pub mod info;
pub mod configuration;
pub mod install_to;
pub mod run;
pub mod run_local;
pub mod serve;

#[derive(Debug, Eq, PartialEq)]
pub enum ConfigStyle {
  CreateMissingKeys,
  DoNotCreateMissingKeys
}

pub async fn run_command(cmd: &args::Command, args: &args::Args) -> DynResult<()> {

  match cmd {
    Command::Info { file_path } => {
      info::info(file_path).await.map_err(map_loc_err!())?;
    }
    Command::Configuration { } => {
      configuration::configuration(args, ConfigStyle::DoNotCreateMissingKeys).await.map_err(map_loc_err!())?;
    }
    Command::GenerateMissingKeys { } => {
      configuration::configuration(args, ConfigStyle::CreateMissingKeys).await.map_err(map_loc_err!())?;
    }
    Command::InstallTo { install_root, install_etc, install_bin } => {
      install_to::install_to(install_root, install_etc, install_bin).await.map_err(map_loc_err!())?;
    }
    Command::Run { file_path, multicast_groups, port } => {
      run::run(args, file_path, multicast_groups.clone(), *port).await.map_err(map_loc_err!())?;
    }
    Command::RunLocal { file_path } => {
      run_local::run_local(file_path, args).await.map_err(map_loc_err!())?;
    }
    Command::Serve { multicast_groups, port } => {
      serve::serve(args, multicast_groups.clone(), *port).await.map_err(map_loc_err!())?;
    }
  }

  Ok(())
}

