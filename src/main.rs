
// TODO remove after development
#![allow(unused_variables, unused_imports, dead_code)]

type DynResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

mod args;
mod config;
mod comm;
mod command;
mod executor;
mod universal_serde;
mod net_utils;
mod messages;
mod crypto_utils;
mod fs_utils;
mod sys_utils;
mod err_utils;

fn main() {
    use clap::Parser;
    let mut args = args::Args::parse();

    GLOBAL_VERBOSITY.set(args.verbosity.into()).expect("Failed to assign GLOBAL_VERBOSITY");
    let log_guard = init_logging();

    if args.v_is_debug() {
        println!("args={:#?}", args);
    }

    let rt  = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build Tokio Runtime");

    match rt.block_on(async_main(&mut args)) {
        Ok(_) => { }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

}

async fn async_main(args: &mut args::Args) -> DynResult<()> {

    handle_friendly_ux_warnings(args).await;

    command::run_command(&args.command, &args).await.map_err(map_loc_err!())?;

    Ok(())
}

async fn handle_friendly_ux_warnings(args: &mut args::Args) {
    if let args::Command::GenerateMissingKeys {} = args.command {
        return;
    }
    match config::Config::read_from_file(&args.config).await.map_err(map_loc_err!()) {
        Ok(local_config) => {
            // Check if a key exists; if not offer to run "Command::GenerateMissingKeys"
            match local_config.identity.read_public_key_ed25519_pem_file().await {
                Ok(_) => { }
                Err(e) => {
                    tracing::info!("WARNING: {}", e);
                    if sys_utils::ask_user_proceed_yn("Would you like to generate a private keypair now? (y/n) ").await {
                        if let Err(e) = command::run_command(&args::Command::GenerateMissingKeys {}, &args).await.map_err(map_loc_err!()) {
                            tracing::info!("ERROR: {}", e);
                        }
                    }
                }
            }
        }
        Err(e) => {
            tracing::info!("WARNING: You have no config file located at {:?} ({})", &args.config, e);
        }
    }
}





/// If we don't want to pay the cost of plumbing args::Args down into a bajillion function calls, we store the verbosity globally.
pub static GLOBAL_VERBOSITY: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
fn get_global_verbosity() -> u8 {
    *GLOBAL_VERBOSITY.get().unwrap_or_else(|| &0u8)
}
pub fn v_is_info() -> bool {
    return get_global_verbosity() > 0;
}
pub fn v_is_debug() -> bool {
    return get_global_verbosity() > 1;
}
pub fn v_is_everything() -> bool {
    return get_global_verbosity() > 2;
}


fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {

    // Set up an async writer to stderr
    let (nb, guard) = tracing_appender::non_blocking(std::io::stderr());

    tracing_subscriber::fmt()
        .with_writer(nb) // Log format config below
        .without_time()
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_level(false)
        .init();

    guard
}

