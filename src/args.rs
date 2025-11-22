
// Weverywhere imlements Display for PathBuf

#[derive(Debug, clap::Parser)]
#[command(
    name = "weverywhere",
    version = "1.0",
    about = "A WASI program management tool supporting the execution of WASI binaries everywhere."
)]
pub struct Args {
    #[command(subcommand)]
    command: Command,

    #[arg(short, long, action = clap::ArgAction::Count)]
    verbosity: u8,

}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Print information about a WASI file, such as function imports and exports
    Info {
        /// Path to the WASI file
        file_path: std::path::PathBuf,
    },

    /// Run the given WASI file
    Run {
        /// Path to the WASI file
        file_path: std::path::PathBuf,

        /// UDP Multicast address to send to
        #[arg(short, long, default_value_t = core::net::IpAddr::V4(std::net::Ipv4Addr::new(224, 0, 0, 2)) )]
        multicast_group: core::net::IpAddr,

        /// UDP Multicast address to send to
        #[arg(short, long, default_value_t = 2240)]
        port: u16,
    },

    /// Listen on the given socket for network messages and execute WASI programs sent to us
    Serve {
        /// UDP Multicast address to listen on
        #[arg(short, long, default_value_t = core::net::IpAddr::V4(std::net::Ipv4Addr::new(224, 0, 0, 2)) )]
        multicast_group: core::net::IpAddr,

        /// UDP Multicast address to listen on
        #[arg(short, long, default_value_t = 2240)]
        port: u16,

        /// Path to the WASI file
        #[arg(short, long, default_value = "/etc/weverywhere.toml" )]
        config: std::path::PathBuf,
    }

}


impl Args {
    pub fn v_is_info(&self) -> bool {
        return self.verbosity > 0;
    }
    pub fn v_is_debug(&self) -> bool {
        return self.verbosity > 1;
    }
    pub fn v_is_everything(&self) -> bool {
        return self.verbosity > 2;
    }
}

