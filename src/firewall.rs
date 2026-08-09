use crate::*;

/// Best-effort: make sure the host firewall lets the serve daemon actually *receive* on UDP `port` -
/// both unicast (`[[peer]]` execute requests) and multicast (discovery / `--fabric`). Without this,
/// a host firewall silently drops inbound datagrams and the node never responds to `netmap`/`run`
/// even though it is bound and listening (this is exactly why a stock Windows guest goes dark while a
/// Linux one answers).
///
/// This is deliberately done by the *server itself* on every `serve` startup, not baked into any
/// deployment script, so any weverywhere node self-heals its own reachability. It is always
/// best-effort: the weverywhere daemon normally runs elevated (SYSTEM via the scheduled task / root
/// via systemd) where this succeeds, but a non-privileged dev `serve` just gets a warning and keeps
/// running (loopback and already-open LANs need no rule). We never fail `serve` over a firewall step.
pub async fn ensure_inbound_udp_allowed(port: u16) {
  match backend::ensure_inbound_udp_allowed(port).await {
    Ok(true) => tracing::info!("[ firewall ] ensured inbound UDP {port} is permitted for weverywhere"),
    Ok(false) => { /* nothing to do on this platform / no active firewall */ }
    Err(e) => tracing::warn!(
      "[ firewall ] could not ensure inbound UDP {port} is allowed ({e}). If this host has a \
       firewall, open UDP {port} inbound (unicast + multicast) so weverywhere can receive."
    ),
  }
}

/// Run a command purely for its exit status, mapping a non-zero exit to an error. Inherits no stdio
/// so firewall tooling chatter does not pollute the daemon's logs.
#[cfg(any(target_os = "windows", target_os = "linux"))]
async fn run_status(program: &str, args: &[&str]) -> DynResult<std::process::ExitStatus> {
  use std::process::Stdio;
  let status = tokio::process::Command::new(program)
    .args(args)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
    .await
    .map_err(map_loc_err!())?;
  Ok(status)
}


// ===========================================================================================
// Windows: Defender Firewall via netsh (works without PowerShell; no SCM plumbing needed)
// ===========================================================================================
#[cfg(target_os = "windows")]
mod backend {
  use super::*;

  /// Add an inbound "allow UDP <port>" rule to Windows Defender Firewall for all profiles. Windows
  /// filters inbound UDP by local port, so one port rule covers both unicast and multicast delivery
  /// to our socket. Idempotent: we delete any existing rule of the same name first, then re-add, so
  /// re-running (every boot) never stacks duplicate rules.
  pub async fn ensure_inbound_udp_allowed(port: u16) -> DynResult<bool> {
    let rule_name = format!("weverywhere (UDP {port} in)");
    let name_arg = format!("name={rule_name}");

    // Best-effort delete of a prior copy (fails harmlessly when none exists).
    let _ = run_status("netsh", &["advfirewall", "firewall", "delete", "rule", &name_arg]).await;

    let localport = format!("localport={port}");
    let status = run_status("netsh", &[
      "advfirewall", "firewall", "add", "rule",
      &name_arg,
      "dir=in", "action=allow", "protocol=UDP",
      &localport,
      "profile=any",
    ]).await?;

    if !status.success() {
      return Err(format!("netsh add rule exited {status}").into());
    }
    Ok(true)
  }
}


// ===========================================================================================
// Linux: firewalld via firewall-cmd, only when it is actually running (best-effort, runtime-only)
// ===========================================================================================
#[cfg(target_os = "linux")]
mod backend {
  use super::*;

  /// If firewalld is active, open UDP `port` in the default zone at runtime. We intentionally do NOT
  /// touch the permanent config (no surprise persistent changes to a user's firewall); the daemon
  /// re-applies this on every boot. Link-local multicast (224.0.0.0/24, ff02::) is already permitted
  /// by firewalld, so a single port opening is enough to start receiving. Hosts without firewalld
  /// (e.g. minimal cloud images) need nothing and report "no active firewall".
  pub async fn ensure_inbound_udp_allowed(port: u16) -> DynResult<bool> {
    // `firewall-cmd --state` exits 0 only when firewalld is running; if the binary is missing the
    // spawn errors, which we treat as "no firewalld here".
    match run_status("firewall-cmd", &["--state"]).await {
      Ok(state) if state.success() => {}
      _ => return Ok(false), // no firewalld / not running -> nothing to do
    }

    let add = format!("--add-port={port}/udp");
    let status = run_status("firewall-cmd", &[&add]).await?;
    if !status.success() {
      return Err(format!("firewall-cmd {add} exited {status}").into());
    }
    Ok(true)
  }
}


// ===========================================================================================
// Other platforms (macOS, BSD, ...): the default host firewall does not block inbound UDP for a
// running daemon in a way we can (or should) portably poke, so this is a no-op.
// ===========================================================================================
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod backend {
  use super::*;
  pub async fn ensure_inbound_udp_allowed(_port: u16) -> DynResult<bool> {
    Ok(false)
  }
}
