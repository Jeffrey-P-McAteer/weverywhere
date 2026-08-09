
// Weverywhere imlements Display for PathBuf

use std::ops::DerefMut;
use std::ops::Deref;
use std::str::FromStr;

#[derive(Debug, clap::Parser)]
#[command(
    name = "weverywhere",
    // Baked in by build.rs (YYYY.MM.<hours-into-month, UTC>); see scripts/_version.py.
    version = env!("WEVERYWHERE_VERSION"),
    about = "A WASI program management tool supporting the execution of WASI binaries everywhere."
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,

    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbosity: u8,

    /// Path to the weverywhere.toml configuration file. When omitted, weverywhere looks next to the
    /// installed binary (<bin>/../etc/weverywhere.toml, the install-to layout) and then falls back
    /// to the platform's system config path (see --help output / config_path()).
    #[arg(short, long)]
    pub config: Option<std::path::PathBuf>,

}

/// The platform's default system-wide config location, used when --config is not given and no
/// config was found next to the binary. weverywhere.toml is machine-wide (the daemon runs as
/// root / SYSTEM), so we use each OS's conventional machine config directory.
pub fn default_config_path() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        // e.g. C:\ProgramData\weverywhere\weverywhere.toml
        let base = std::env::var_os("ProgramData")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
        return base.join("weverywhere").join("weverywhere.toml");
    }
    #[cfg(target_os = "macos")]
    {
        return std::path::PathBuf::from("/Library/Application Support/weverywhere/weverywhere.toml");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        return std::path::PathBuf::from("/etc/weverywhere.toml");
    }
}

/// The config that `install-to <root>` stages next to the binary (<bin>/../etc/weverywhere.toml).
/// Returned only when it actually exists so discovery can prefer it over the system default.
fn exe_relative_config() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("..").join("etc").join("weverywhere.toml");
    std::fs::canonicalize(candidate).ok()
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

    /// Install weverywhere into a filesystem tree: extract the embedded etc/ config templates and
    /// copy this binary under INSTALL_ROOT (into etc/ and bin/ by default).
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

    /// Run the given WASI file. By default this talks to the local daemon on this machine;
    /// pass --fabric to instead broadcast the request to the multicast fabric (the LAN).
    Run {
        /// Path to the WASI file
        file_path: std::path::PathBuf,

        /// Broadcast to the whole multicast fabric instead of only the local daemon
        #[arg(short, long, default_value_t = false)]
        fabric: bool,

        /// UDP Multicast addresses to send to (only used with --fabric)
        #[arg(short, long, default_value_t = default_multicast_groups() )]
        multicast_groups: MulticastAddressVec,

        /// UDP port the daemon listens on
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

    },

    /// Discover the weverywhere fabric and draw a trust-annotated map of it. This sends a WASI
    /// discovery *program* to every reachable server (the whole multicast fabric by default, or
    /// only the local daemon with --local), collects the hostname + observed peers each server
    /// reports back, and prints the resulting network as a tree. Discovery is deliberately a
    /// program, not a wire message, so richer topology/telemetry can ship as different programs.
    Netmap {
        /// Path to the discovery WASI program. Defaults to the compiled network-map example
        /// (target/example-programs/network-map.wasm); build it with
        /// `uv run scripts/compile-example-programs.py`.
        #[arg(long)]
        program: Option<std::path::PathBuf>,

        /// Query only the local daemon on this machine instead of the whole multicast fabric.
        #[arg(short, long, default_value_t = false)]
        local: bool,

        /// UDP Multicast addresses to send the discovery program to
        #[arg(short, long, default_value_t = default_multicast_groups() )]
        multicast_groups: MulticastAddressVec,

        /// UDP port the daemon listens on
        #[arg(short, long, default_value_t = 2240)]
        port: u16,
    },

    /// Manage weverywhere as a long-running background daemon (OS service) that runs `serve`.
    /// Uses the native service manager on each platform: systemd on Linux, launchd on macOS,
    /// and the Task Scheduler on Windows. Most actions must run as root / Administrator.
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    }

}

#[derive(Debug, Clone, clap::Subcommand)]
pub enum DaemonAction {
    /// Register weverywhere as a boot-time daemon and start it now
    Install,
    /// Stop the daemon and remove its service registration
    Uninstall,
    /// Start the daemon now
    Start,
    /// Stop the daemon now
    Stop,
    /// Restart the daemon now
    Restart,
    /// Print whether the daemon is installed and running
    Status,
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
    /// Resolve the config file path across platforms:
    ///   1. an explicit `--config <path>` always wins;
    ///   2. otherwise a config staged next to the binary (install-to layout), if present;
    ///   3. otherwise the platform's default system config path.
    pub fn config_path(&self) -> std::path::PathBuf {
        if let Some(explicit) = &self.config {
            return explicit.clone();
        }
        if let Some(found) = exe_relative_config() {
            return found;
        }
        default_config_path()
    }

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

