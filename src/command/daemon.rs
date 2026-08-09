
use super::*;

/// Manage weverywhere as a native OS-managed daemon. Whatever backend a platform uses
/// (systemd / launchd / Task Scheduler), the same six verbs are supported so the binary can
/// install, start, stop, restart, uninstall, and report the status of itself as a daemon.
///
/// The registered daemon runs `<this-exe> serve`, so `install-to` the binary first (or just run
/// it from wherever it lives) and then `daemon install`. All mutating actions need root on
/// unix / Administrator on Windows.
pub async fn daemon(action: &DaemonAction, _args: &args::Args) -> DynResult<()> {
  match action {
    DaemonAction::Install   => {
      backend::install().await?;
      // The installed daemon runs `serve` (no --port), so open the host firewall for serve's
      // default UDP port now, while we already hold the elevation `install` required. `serve` also
      // re-ensures this at startup; doing it here means the node is reachable the moment it starts.
      crate::firewall::ensure_inbound_udp_allowed(DEFAULT_SERVE_PORT).await;
      Ok(())
    }
    DaemonAction::Uninstall => backend::uninstall().await,
    DaemonAction::Start     => backend::start().await,
    DaemonAction::Stop      => backend::stop().await,
    DaemonAction::Restart   => backend::restart().await,
    DaemonAction::Status    => backend::status().await,
  }
}

/// UDP port the installed daemon will `serve` on. Mirrors the `--port` default of the `Serve`
/// subcommand in `args.rs`; `serve_args_vec()` deliberately passes no `--port`, so the daemon always
/// uses this default - and this is therefore the port to open in the firewall at install time.
const DEFAULT_SERVE_PORT: u16 = 2240;

/// Absolute path to the currently running executable - this is what the daemon runs at boot.
fn this_exe() -> DynResult<std::path::PathBuf> {
  Ok(std::env::current_exe()?)
}

/// If `install-to` placed a config next to the binary (`<bin>/../etc/weverywhere.toml`), return it
/// so the installed daemon is pointed at it. Otherwise the daemon relies on `serve`'s default
/// config path (e.g. /etc/weverywhere.toml). Keeps `install-to <root>` + `daemon install` coherent.
fn installed_config_path() -> Option<std::path::PathBuf> {
  let exe = std::env::current_exe().ok()?;
  let candidate = exe.parent()?.join("..").join("etc").join("weverywhere.toml");
  std::fs::canonicalize(candidate).ok()
}

/// The argv tail the daemon should run: `serve`, prefixed with `--config <path>` when install-to
/// staged a config next to the binary.
fn serve_args_vec() -> Vec<String> {
  match installed_config_path() {
    Some(cfg) => vec!["--config".into(), cfg.display().to_string(), "serve".into()],
    None => vec!["serve".into()],
  }
}

/// Run a command, inheriting stdio so its output is visible; error on a non-zero exit.
async fn sh(program: &str, args: &[&str]) -> DynResult<()> {
  tracing::info!("+ {} {}", program, args.join(" "));
  let status = tokio::process::Command::new(program).args(args).status().await.map_err(map_loc_err!())?;
  if !status.success() {
    return Err(format!("`{} {}` failed ({})", program, args.join(" "), status).into());
  }
  Ok(())
}

/// Run a command for its output only (status queries) - never treated as an error.
async fn sh_show(program: &str, args: &[&str]) -> DynResult<()> {
  let _ = tokio::process::Command::new(program).args(args).status().await;
  Ok(())
}


// ===========================================================================================
// Linux: systemd
// ===========================================================================================
#[cfg(target_os = "linux")]
mod backend {
  use super::*;

  const SERVICE: &str = "weverywhere";
  const UNIT_PATH: &str = "/etc/systemd/system/weverywhere.service";

  pub async fn install() -> DynResult<()> {
    let exe = this_exe()?;
    let exec_start = format!("{} {}", exe.display(), serve_args_vec().join(" "));
    let unit = format!(
"[Unit]
Description=weverywhere WASI fabric daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={exec_start}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
");
    tokio::fs::write(UNIT_PATH, unit).await
      .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        format!("writing {UNIT_PATH} failed ({e}); daemon install must run as root").into()
      })?;
    sh("systemctl", &["daemon-reload"]).await?;
    sh("systemctl", &["enable", "--now", SERVICE]).await?;
    println!("Installed and started the weverywhere systemd service ({SERVICE}).");
    Ok(())
  }

  pub async fn uninstall() -> DynResult<()> {
    let _ = sh("systemctl", &["disable", "--now", SERVICE]).await; // best effort
    let _ = tokio::fs::remove_file(UNIT_PATH).await;
    sh("systemctl", &["daemon-reload"]).await?;
    println!("Removed the weverywhere systemd service.");
    Ok(())
  }

  pub async fn start() -> DynResult<()> { sh("systemctl", &["start", SERVICE]).await }
  pub async fn stop() -> DynResult<()> { sh("systemctl", &["stop", SERVICE]).await }
  pub async fn restart() -> DynResult<()> { sh("systemctl", &["restart", SERVICE]).await }
  pub async fn status() -> DynResult<()> { sh_show("systemctl", &["status", "--no-pager", SERVICE]).await }
}


// ===========================================================================================
// macOS: launchd
// ===========================================================================================
#[cfg(target_os = "macos")]
mod backend {
  use super::*;

  const LABEL: &str = "com.weverywhere.daemon";
  const PLIST_PATH: &str = "/Library/LaunchDaemons/com.weverywhere.daemon.plist";

  pub async fn install() -> DynResult<()> {
    let exe = this_exe()?;
    // Build the <string>...</string> ProgramArguments: the exe followed by the serve argv tail.
    let mut prog_args = format!("    <string>{}</string>\n", exe.display());
    for a in serve_args_vec() {
      prog_args.push_str(&format!("    <string>{a}</string>\n"));
    }
    let plist = format!(
"<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{prog_args}  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/var/log/weverywhere.log</string>
  <key>StandardErrorPath</key><string>/var/log/weverywhere.log</string>
</dict>
</plist>
", label = LABEL, prog_args = prog_args);
    tokio::fs::write(PLIST_PATH, plist).await
      .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        format!("writing {PLIST_PATH} failed ({e}); daemon install must run as root").into()
      })?;
    sh("launchctl", &["load", "-w", PLIST_PATH]).await?;
    println!("Installed and started the weverywhere launchd daemon ({LABEL}).");
    Ok(())
  }

  pub async fn uninstall() -> DynResult<()> {
    let _ = sh("launchctl", &["unload", "-w", PLIST_PATH]).await; // best effort
    let _ = tokio::fs::remove_file(PLIST_PATH).await;
    println!("Removed the weverywhere launchd daemon.");
    Ok(())
  }

  pub async fn start() -> DynResult<()> { sh("launchctl", &["start", LABEL]).await }
  pub async fn stop() -> DynResult<()> { sh("launchctl", &["stop", LABEL]).await }
  pub async fn restart() -> DynResult<()> {
    let _ = sh("launchctl", &["stop", LABEL]).await;
    sh("launchctl", &["start", LABEL]).await
  }
  pub async fn status() -> DynResult<()> { sh_show("launchctl", &["list", LABEL]).await }
}


// ===========================================================================================
// Windows: Task Scheduler (works with a plain console exe; no SCM service plumbing required)
// ===========================================================================================
#[cfg(target_os = "windows")]
mod backend {
  use super::*;

  const TASK: &str = "weverywhere";

  pub async fn install() -> DynResult<()> {
    let exe = this_exe()?;
    let run = format!("\"{}\" {}", exe.display(), serve_args_vec().join(" "));
    sh("schtasks", &["/create", "/tn", TASK, "/tr", run.as_str(),
                     "/sc", "onstart", "/ru", "SYSTEM", "/rl", "HIGHEST", "/f"]).await?;
    let _ = sh("schtasks", &["/run", "/tn", TASK]).await; // start now (best effort)
    println!("Installed and started the weverywhere scheduled task ({TASK}).");
    Ok(())
  }

  pub async fn uninstall() -> DynResult<()> {
    let _ = sh("schtasks", &["/end", "/tn", TASK]).await; // best effort
    sh("schtasks", &["/delete", "/tn", TASK, "/f"]).await?;
    println!("Removed the weverywhere scheduled task.");
    Ok(())
  }

  pub async fn start() -> DynResult<()> { sh("schtasks", &["/run", "/tn", TASK]).await }
  pub async fn stop() -> DynResult<()> { sh("schtasks", &["/end", "/tn", TASK]).await }
  pub async fn restart() -> DynResult<()> {
    let _ = sh("schtasks", &["/end", "/tn", TASK]).await;
    sh("schtasks", &["/run", "/tn", TASK]).await
  }
  pub async fn status() -> DynResult<()> { sh_show("schtasks", &["/query", "/tn", TASK, "/v", "/fo", "list"]).await }
}


// ===========================================================================================
// Any other platform: no daemon backend
// ===========================================================================================
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod backend {
  use super::*;

  fn unsupported() -> DynResult<()> {
    Err("daemon management is not implemented for this platform".into())
  }
  pub async fn install() -> DynResult<()> { unsupported() }
  pub async fn uninstall() -> DynResult<()> { unsupported() }
  pub async fn start() -> DynResult<()> { unsupported() }
  pub async fn stop() -> DynResult<()> { unsupported() }
  pub async fn restart() -> DynResult<()> { unsupported() }
  pub async fn status() -> DynResult<()> { unsupported() }
}
