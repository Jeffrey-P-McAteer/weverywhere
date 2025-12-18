
// Weverywhere imlements Display for PathBuf

use std::ops::DerefMut;
use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, clap::Parser)]
#[command(
    name = "weverywhere",
    version = "1.0",
    about = "A WASI program management tool supporting the execution of WASI binaries everywhere."
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,

    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbosity: u8,

    /// Path to the WASI file
    #[arg(short, long, default_value = "/etc/weverywhere.toml" )]
    pub config: std::path::PathBuf,

}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Print information about a WASI file, such as function imports and exports
    Info {
        /// Path to the WASI file
        file_path: std::path::PathBuf,
    },

    /// Prints information about your current configuration;
    Configuration {

    },

    /// Generates any private keys found in configuration which do not exist;
    GenerateMissingKeys {

    },

    InstallTo {
        /// Path to root of system to install into.
        /// This generally must run as root and will write to files under etc/ and bin/
        /// within the folder, unless --install-etc and or --install-bin are passed.
        install_root: std::path::PathBuf,

        /// An override for the path to etc relative to INSTALL_ROOT
        #[arg(long, default_value = "etc")]
        install_etc: std::path::PathBuf,

        /// An override for the path to bin relative to INSTALL_ROOT
        #[arg(long, default_value = "bin")]
        install_bin: std::path::PathBuf,
    },

    /// Run the given WASI file
    Run {
        /// Path to the WASI file
        file_path: std::path::PathBuf,

        /// UDP Multicast addresses to send to
        #[arg(short, long, default_value_t = default_multicast_groups() )]
        multicast_groups: MulticastAddressVec,

        /// UDP Multicast address to send to
        #[arg(short, long, default_value_t = 2240)]
        port: u16,
    },

    /// Run the given WASI file locally, spinning up an executor as-if we had just become a server and recieved the program.
    // Primarially for debugging, local testing, etc. Reads the same --config file as "serve" does.
    RunLocal {
        /// Path to the WASI file
        file_path: std::path::PathBuf,
    },

    /// Listen on the given socket for network messages and execute WASI programs sent to us
    Serve {
        /// UDP Multicast addresses to listen on
        #[arg(short, long, default_value_t = default_multicast_groups() )]
        multicast_groups: MulticastAddressVec,

        /// UDP Multicast address to listen on
        #[arg(short, long, default_value_t = 2240)]
        port: u16,

    }

}

fn default_multicast_groups() -> MulticastAddressVec {
    let mut groups = Vec::with_capacity(2);
    groups.push(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
        // "Unassigned" per https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
        224, 0, 0, 3
    )));
    groups.push(std::net::IpAddr::V6(std::net::Ipv6Addr::new(
        // "Unassigned" per https://www.iana.org/assignments/ipv6-multicast-addresses/ipv6-multicast-addresses.xhtml
        0xFF02, 0x0000, 0x0000, 0x0000,
        0x0000, 0x0000, 0x0000, 0x0003
    )));
    MulticastAddressVec(groups)
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

#[derive(Debug, Clone)]
pub struct MulticastAddressVec(Vec<std::net::IpAddr>);

impl std::fmt::Display for MulticastAddressVec {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let fmt_output: String = self.0.iter()
                             .map(|addr| format!("{}", addr) )
                             .collect::<Vec<_>>() // replace these 2 lines w/ commented ones when intersperse becomes stable!
                             .join(",");
                             //.intersperse(",".to_string())
                             //.collect();

        write!(f, "{}", fmt_output)
    }
}

impl std::str::FromStr for MulticastAddressVec {
    type Err = Box<dyn std::error::Error>;
    fn from_str(s: &str) -> Result<Self, <Self as std::str::FromStr>::Err> {
        let mut groups = Vec::with_capacity(4);
        for part in s.split([' ', ',']) {
            match std::net::IpAddr::from_str(part) {
                Ok(addr) => {
                    if addr.is_multicast() {
                        groups.push(addr);
                    }
                    else {
                        tracing::warn!("WARNING: Ignoring non-multicast address {}", addr);
                    }
                }
                Err(e) => {
                    tracing::warn!("Error: {:?}", e);
                }
            }
        }
        if groups.len() > 0 {
            Ok(MulticastAddressVec(groups))
        }
        else {
            Err(format!("Error: {} did not specify ANY multicast addresses", s).into())
        }
    }
}

impl From<String> for MulticastAddressVec {
    fn from(s: std::string::String) -> Self {
        match MulticastAddressVec::from_str(&s) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::warn!("{:?}", e);
                default_multicast_groups()
            }
        }
    }
}

impl IntoIterator for MulticastAddressVec {
  type Item = std::net::IpAddr;
  type IntoIter = <Vec<std::net::IpAddr> as IntoIterator>::IntoIter; // so that you don't have to write std::vec::IntoIter, which nobody remembers anyway

  fn into_iter(self) -> Self::IntoIter {
    self.0.into_iter()
  }
}

// We deref to slice so that we can reuse the slice impls
impl Deref for MulticastAddressVec {
  type Target = [std::net::IpAddr];

  fn deref(&self) -> &[std::net::IpAddr] {
    &self.0[..]
  }
}
impl DerefMut for MulticastAddressVec {
  fn deref_mut(&mut self) -> &mut [std::net::IpAddr] {
    &mut self.0[..]
  }
}

