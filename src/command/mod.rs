
use crate::*;
use crate::args::*;

pub mod info;
pub mod configuration;
pub mod install_to;
pub mod daemon;
pub mod run;
pub mod run_local;
pub mod serve;
pub mod netmap;
pub mod chat;
pub mod extract_programs;

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
    Command::Run { file_path, fabric, multicast_groups, port, arg, arg_list } => {
      let arg_map = args::parse_arg_map(arg);
      run::run(args, file_path, *fabric, multicast_groups.clone(), *port, arg_list.clone(), arg_map).await.map_err(map_loc_err!())?;
    }
    Command::RunLocal { file_path, arg, arg_list, multicast_groups, port } => {
      let arg_map = args::parse_arg_map(arg);
      run_local::run_local(file_path, args, arg_list.clone(), arg_map, multicast_groups.clone(), *port).await.map_err(map_loc_err!())?;
    }
    Command::Serve { multicast_groups, port } => {
      serve::serve(args, multicast_groups.clone(), *port).await.map_err(map_loc_err!())?;
    }
    Command::Netmap { program, local, multicast_groups, port } => {
      netmap::netmap(args, program.clone(), *local, multicast_groups.clone(), *port).await.map_err(map_loc_err!())?;
    }
    Command::Chat { program, multicast_groups, port } => {
      chat::chat(args, program.clone(), multicast_groups.clone(), *port).await.map_err(map_loc_err!())?;
    }
    Command::ExtractPrograms { out_dir } => {
      extract_programs::extract_programs(out_dir).await.map_err(map_loc_err!())?;
    }
    Command::Daemon { action } => {
      daemon::daemon(action, args).await.map_err(map_loc_err!())?;
    }
  }

  Ok(())
}

