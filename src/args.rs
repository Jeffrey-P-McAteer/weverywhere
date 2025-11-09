
#[derive(Debug, clap::Parser)]
#[command(
    name = "weverywhere",
    version = "1.0",
    about = "A source-code, executable-binary, and web-url security information gathering and reporting utility."
)]
pub struct Args {
    /// TODO design input API
    pub input: Option<String>,

}


