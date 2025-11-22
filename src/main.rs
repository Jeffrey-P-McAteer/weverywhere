
// TODO remove after development
#![allow(unused_variables, unused_imports, dead_code)]


type DynResult<T> = Result<T, Box<dyn std::error::Error>>;

mod args;
mod config;
mod comm;

fn main() {
    use clap::Parser;
    let args = args::Args::parse();

    if args.v_is_debug() {
        println!("args={:?}", args);
    }



}

