
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
      info::info(file_path).await?;
    }
    Command::Configuration { } => {
      configuration::configuration(args, ConfigStyle::DoNotCreateMissingKeys).await?;
    }
    Command::GenerateMissingKeys { } => {
      configuration::configuration(args, ConfigStyle::CreateMissingKeys).await?;
    }
    Command::InstallTo { install_root, install_etc, install_bin } => {
      install_to::install_to(install_root, install_etc, install_bin).await?;
    }
    Command::Run { file_path, multicast_groups, port } => {
      run::run(file_path, multicast_groups.clone(), *port).await?;
    }
    Command::RunLocal { file_path } => {
      run_local::run_local(file_path).await?;
    }
    Command::Serve { multicast_groups, port } => {
      serve::serve(multicast_groups.clone(), *port).await?;
    }
  }

  Ok(())
}

