
// TODO remove after development
#![allow(unused_variables, unused_imports, dead_code)]


type DynResult<T> = Result<T, Box<dyn std::error::Error>>;

mod args;
mod config;
mod comm;
mod command;

fn main() {
    use clap::Parser;
    let mut args = args::Args::parse();

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

