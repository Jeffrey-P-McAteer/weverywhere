
// TODO remove after development
#![allow(unused_variables, unused_imports, dead_code)]


type DynResult<T> = Result<T, Box<dyn std::error::Error>>;

mod args;
mod config;
mod comm;
mod command;
mod universal_serde;
mod net_utils;

fn main() {
    use clap::Parser;
    let mut args = args::Args::parse();

    GLOBAL_VERBOSITY.store(args.verbosity.into(), std::sync::atomic::Ordering::Relaxed);

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
            eprintln!("{:?}", e);
            std::process::exit(1);
        }
    }

}

async fn async_main(args: &mut args::Args) -> DynResult<()> {

    command::run_command(&args.command, &args).await?;

    Ok(())
}







/// If we don't want to pay the cost of plumbing args::Args down into a bajillion function calls, we store the verbosity globally.
pub static GLOBAL_VERBOSITY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
fn get_global_verbosity() -> u8 {
    GLOBAL_VERBOSITY.load(std::sync::atomic::Ordering::Relaxed) as u8
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


