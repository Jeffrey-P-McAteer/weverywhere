use super::*;

/// Emit an always-on, structured security-audit line for a signed fabric event, so external log
/// consumers can track identities by their unforgeable FULL public key and flag imposters across
/// every application on the fabric. `short` is the same 8-hex short-id shown in chat and netmap
/// (`crypto_utils::short_id`); `pubkey` is the complete ed25519 key. A GOOD verdict logs at info, a
/// BAD one (forged identity, tampering, or replay) at warn so alerts stand out - but both carry the
/// `weverywhere::security` target and a `[security]` tag so a SIEM can capture the full stream.
/// These are NOT gated on verbosity: security auditing must be reliable regardless of `-v`.
fn log_sig_event(kind: &str, ok: bool, addr: std::net::SocketAddr, source: &config::IdentityData, detail: &str) {
  let short = crypto_utils::short_id(&source.encoded_public_key);
  let pubkey = crypto_utils::to_hex(&source.encoded_public_key);
  let verdict = if ok { "GOOD" } else { "BAD" };
  let name = &source.human_name;
  if ok {
    tracing::info!(target: "weverywhere::security",
      "[security] sig={verdict} kind={kind} addr={addr} short={short} pubkey={pubkey} name={name:?}{detail}");
  } else {
    tracing::warn!(target: "weverywhere::security",
      "[security] sig={verdict} kind={kind} addr={addr} short={short} pubkey={pubkey} name={name:?}{detail}");
  }
}

#[allow(unreachable_code)]
pub async fn serve(args: &args::Args, multicast_group: args::MulticastAddressVec, port: u16) -> DynResult<()> {

  let local_config = config::Config::read_from_file(&args.config_path()).await.map_err(map_loc_err!())?;
  let executor = executor::Executor::new(&local_config).await;

  // Make sure the host firewall actually lets us receive on this UDP port (unicast + multicast)
  // before we start listening. Best-effort and never fatal - see firewall::ensure_inbound_udp_allowed.
  firewall::ensure_inbound_udp_allowed(port).await;

  // Shared, read-only view of our config for the discovery relay (the [[peer]] forwarding targets).
  let local_config = std::sync::Arc::new(local_config);

  let tasks = spawn_listeners(executor, local_config, multicast_group, port);
  tasks.join_all().await;

  Ok(())
}

/// Spawn one UDP listener task per (interface x multicast group) on the given executor, returning the
/// JoinSet. Both `serve` and the self-contained `chat` host use this so a single executor (and its
/// shared message store) backs all inbound fabric traffic. The caller decides whether to join the set
/// (block, like `serve`) or let it run in the background (like `chat`, which also drives a UI).
pub fn spawn_listeners(
  executor: std::sync::Arc<executor::Executor>,
  local_config: std::sync::Arc<config::Config>,
  multicast_group: args::MulticastAddressVec,
  port: u16,
) -> tokio::task::JoinSet<()> {
  let mut tasks = tokio::task::JoinSet::new();
  // One listener per multicast group (NOT per interface). A single reuse-bound socket joins the group
  // on EVERY interface that has an address of the right family, so we receive multicast no matter which
  // NIC it arrives on. The old per-(interface x group) design bound the same 0.0.0.0:port without
  // SO_REUSEADDR, so every interface after the first failed with AddrInUse (silently), leaving the
  // group joined on just one - usually the wrong - interface on multi-NIC / bridge+tap hosts.
  let interfaces = std::sync::Arc::new(net_utils::get_interfaces());
  for multicast_addr in multicast_group.iter() {
    let multicast_addr = multicast_addr.clone();
    let interfaces = interfaces.clone();
    let executor = executor.clone();
    let local_config = local_config.clone();
    tasks.spawn(async move {
      if let Err(e) = serve_group(&interfaces, &multicast_addr, port, executor, local_config).await {
        tracing::warn!("[ serve_group ] Error serving group {} port {}: {:?}", multicast_addr, port, e);
      }
    });
  }
  tasks
}

/// Bind a UDP socket on `0.0.0.0:port` (v4) or `[::]:port` (v6) with address/port reuse enabled, so
/// several listeners (both families, this process's `chat` + a sibling `serve`, or another instance)
/// can share the group+port instead of racing for an exclusive bind. The v6 socket is set v6-only so
/// it doesn't collide with the v4 socket on the same port.
fn bind_reuse_udp(is_v4: bool, port: u16) -> DynResult<tokio::net::UdpSocket> {
  use socket2::{Domain, Protocol, Socket, Type};
  let domain = if is_v4 { Domain::IPV4 } else { Domain::IPV6 };
  let sock = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP)).map_err(map_loc_err!())?;
  sock.set_reuse_address(true).map_err(map_loc_err!())?; // let both families' listeners (and a sibling instance) share the group+port
  let bind_addr: std::net::SocketAddr = if is_v4 {
    (std::net::Ipv4Addr::UNSPECIFIED, port).into()
  } else {
    sock.set_only_v6(true).map_err(map_loc_err!())?;
    (std::net::Ipv6Addr::UNSPECIFIED, port).into()
  };
  sock.set_nonblocking(true).map_err(map_loc_err!())?;
  sock.bind(&bind_addr.into()).map_err(map_loc_err!())?;
  Ok(tokio::net::UdpSocket::from_std(sock.into()).map_err(map_loc_err!())?)
}

#[allow(unreachable_code)]
pub async fn serve_group(interfaces: &[(u32, String, Vec<std::net::IpAddr>)], multicast_addr: &std::net::IpAddr, port: u16, executor: std::sync::Arc<executor::Executor>, local_config: std::sync::Arc<config::Config>) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

  let sock = bind_reuse_udp(multicast_addr.is_ipv4(), port)?;
  let sock = std::sync::Arc::new(sock); // Allow us to send socket to many threads - we wrap in UdpSocketSender as recieving is racy from other threads!

  if crate::v_is_info() {
    tracing::warn!("Successfully bound group {} on port {}", multicast_addr, port);
  }

  // Join the group on every interface that carries an address (0 addresses = no connection, skip). A
  // single socket can hold many memberships; joining them all is what lets us receive multicast that
  // lands on any NIC. Per-interface join failures are logged but not fatal - one bad interface must not
  // sink the whole listener (losing packets is acceptable; the fabric relies on retries).
  let mut joined_any = false;
  match multicast_addr {
    std::net::IpAddr::V4(group) => {
      sock.set_multicast_loop_v4(true).map_err(map_loc_err!())?;
      sock.set_multicast_ttl_v4(4).map_err(map_loc_err!())?; // How many hops multicast can live for - default is just the immediate LAN we are attached to. TODO configure me from /etc/weveryware.toml l8ter
      for (_iface_idx, iface_name, iface_addrs) in interfaces.iter() {
        for iface_addr in iface_addrs.iter() {
          if let std::net::IpAddr::V4(iface_addr_v4) = iface_addr {
            match sock.join_multicast_v4(*group, *iface_addr_v4) {
              Ok(()) => { joined_any = true; if crate::v_is_info() { tracing::warn!("Joined {} on {} ({})", group, iface_name, iface_addr_v4); } }
              Err(e) => { if crate::v_is_info() { tracing::warn!("Join {} on {} ({}) failed: {:?}", group, iface_name, iface_addr_v4, e); } }
            }
          }
        }
      }
      if !joined_any {
        // No usable per-interface address; fall back to the default-route interface so we still receive.
        sock.join_multicast_v4(*group, core::net::Ipv4Addr::UNSPECIFIED).map_err(map_loc_err!())?;
        joined_any = true;
      }
    }
    std::net::IpAddr::V6(group) => {
      sock.set_multicast_loop_v6(true).map_err(map_loc_err!())?;
      for (iface_idx, iface_name, iface_addrs) in interfaces.iter() {
        if iface_addrs.is_empty() { continue; }
        match sock.join_multicast_v6(group, *iface_idx) {
          Ok(()) => { joined_any = true; if crate::v_is_info() { tracing::warn!("Joined {} on {} (idx {})", group, iface_name, iface_idx); } }
          Err(e) => { if crate::v_is_info() { tracing::warn!("Join {} on {} (idx {}) failed: {:?}", group, iface_name, iface_idx, e); } }
        }
      }
    }
  }
  if !joined_any {
    tracing::warn!("[ serve_group ] joined group {} on NO interfaces - will not receive multicast", multicast_addr);
  }

    // A whole program arrives in one datagram, so this must be large enough to hold the biggest
    // ExecuteRequest we expect. UDP tops out near 64KiB; size to that so we don't silently truncate
    // (a too-small buffer clips the request and serde_bare fails with UnexpectedEof).
    let mut buf = [0; 64*1024];
    loop {
        let (len, addr) = sock.recv_from(&mut buf).await.map_err(map_loc_err!())?;
        //tracing::warn!("{:?} bytes received from {:?} => {:?}", len, addr, &buf[..len]);
        if crate::v_is_everything() {
          tracing::warn!("{:?} bytes received from {:?} => {:?}", len, addr, &buf[..len]);
        }
        else if crate::v_is_info() {
          tracing::warn!("{:?} bytes received from {:?}", len, addr);
        }

        #[allow(unreachable_patterns)]
        match serde_bare::from_slice::<messages::NetworkMessage>(&buf[..len]) {
          Ok(network_message) => {
            match network_message {
              messages::NetworkMessage::ExecuteRequest { program_data } => {
                // Security audit: record who asked us to run a program, by full public key, and whether
                // their identity self-signature checks out. (Whether the program is actually allowed to
                // DO anything is enforced later via host::trusts_me against our trusted-keys set.)
                match program_data.source.check_self_signature() {
                  Ok(()) => log_sig_event("execute-req", true, addr, &program_data.source,
                    &format!(" program={:?}", program_data.human_name)),
                  Err(e) => log_sig_event("execute-req", false, addr, &program_data.source,
                    &format!(" program={:?} error=bad-identity-sig detail={e:?}", program_data.human_name)),
                }
                if crate::v_is_info() {
                  tracing::warn!("Recieved ExecuteRequest: {:?}", &program_data.human_name );
                }
                // Passively remember whoever just contacted us as a fabric neighbour. This is not a
                // discovery protocol: it just lets later discovery *programs* enumerate our peers.
                executor.note_peer(addr, &program_data.source);
                let return_slot = std::sync::Arc::new(std::sync::Mutex::new(executor::ExecReturn::default()));
                let stdio_fwd = executor::wasi_adapters::WasiStdioSimpleForwarder::new_udp(addr, UdpSocketSender::new(&sock) );
                // Our OWN address as this caller reaches us: the local interface address on the same
                // network as `addr`, paired with our serve port. Discovery reports this so the origin
                // sees each node's real address instead of the relay it was reached through.
                let node_addr = net_utils::local_addr_facing(addr.ip())
                  .map(|ip| std::net::SocketAddr::new(ip, port).to_string());
                let exec_opts = executor::ExecOptions { node_addr, ..Default::default() };
                match executor.begin_exec(&program_data, stdio_fwd, exec_opts, return_slot.clone()).await { // TODO async off to a thread pool
                  Ok(running_pid) => {
                    if crate::v_is_info() {
                      tracing::info!("Spawned PID {}", running_pid);
                    }
                    // TODO stdio stuff here?
                    let exit_code = executor.wait_for_pid_exit(running_pid).await?;
                    if crate::v_is_info() {
                      tracing::info!("Exited with code {}", exit_code);
                    }

                    // If the program returned a structured CBOR record (discovery), send it to the
                    // caller as a BasicReturnMap, then recurse: forward the program to our peers and
                    // relay their replies back up (rewriting the UUID to the caller's).
                    let exec_return = return_slot.lock().map(|g| g.clone()).unwrap_or_default();
                    if let Some(cbor) = exec_return.map {
                      let node_msg = messages::NetworkMessage::BasicReturnMap {
                        from_pid: running_pid,
                        request_uuid: program_data.request_uuid,
                        cbor_data: cbor,
                      };
                      if let Ok(enc) = serde_bare::to_vec(&node_msg) {
                        let _ = sock.send_to(&enc, addr).await;
                      }
                    }
                    if program_data.depth_budget > 0 {
                      tokio::spawn(discovery_forward(
                        executor.clone(), local_config.clone(), program_data.clone(),
                        exec_return.forward_uuid, addr, sock.clone(), port,
                      ));
                    }

                    let program_exit_msg = messages::NetworkMessage::BasicInsecureProgramExit {
                      from_pid: running_pid,
                      exit_code: exit_code,
                    };
                    match serde_bare::to_vec(&program_exit_msg) {
                      Ok(program_exit_msg_encoded) => {
                        let len = sock.send_to(&program_exit_msg_encoded, addr).await.map_err(map_loc_err!())?;
                        tracing::warn!("{:?} bytes sent to {:?}", len, addr);
                      }
                      Err(e) => {
                        tracing::info!("e = {:?}", e);
                      }
                    }

                  }
                  Err(e) => {
                    tracing::info!("e = {:?}", e);
                  }
                }
              }
              messages::NetworkMessage::SignedFabricMessage { source, id, cbor_data, signature } => {
                // A lightweight signed message (no program shipped). Verify BOTH the sender's identity
                // self-signature AND the payload signature before trusting the name/pubkey, then append
                // to our store for the local UI to read. Bad signatures are dropped, not executed.
                if let Err(e) = source.check_self_signature() {
                  // Identity self-signature failed: the claimed name/pubkey are unverified (a forged
                  // identity). Log the CLAIMED key anyway so imposters using it are tracked.
                  log_sig_event("fabric-msg", false, addr, &source, &format!(" error=bad-identity-sig detail={e:?}"));
                } else if let Err(e) = source.verify_payload(&id, &cbor_data, &signature) {
                  // Identity is genuine but the payload signature doesn't match: tampering or replay
                  // under this key.
                  log_sig_event("fabric-msg", false, addr, &source, &format!(" error=bad-payload-sig detail={e:?}"));
                } else {
                  log_sig_event("fabric-msg", true, addr, &source, "");
                  executor.note_peer(addr, &source);
                  let epoch = sys_utils::epoch_seconds_now_utc0();
                  let seq = executor.record_fabric_message(source.human_name.clone(), source.encoded_public_key.clone(), &id, cbor_data, epoch);
                  if crate::v_is_info() { tracing::info!("SignedFabricMessage from {:?} stored as seq {}", source.human_name, seq); }
                }
              }
              unused => {
                tracing::warn!("Got unexpected network message: {:?}", unused);
              }
            }
          }
          Err(e) => {
            tracing::warn!("{:?}", e);
          }
        }

        //sock.connect(addr).await.map_err(map_loc_err!())?;  // forces routing decision on BSD and MacOS machines, which otherwise error during send_to with "Os { code: 49, kind: AddrNotAvailable, message: "Can't assign requested address" }"

        // let len = sock.send_to(&buf[..len], addr).await.map_err(map_loc_err!())?;
        // tracing::warn!("{:?} bytes sent", len);

    }

  Ok(())
}


/// Stop a subtree relay after this long with no new data (the subtree has gone quiet).
const RELAY_QUIET_TIMEOUT_MS: u64 = 1500;
/// Absolute cap for one hop's relay window (the origin client waits ~12s overall).
const RELAY_MAX_MS: u64 = 9000;

/// Forward the discovery program to each eligible peer and relay their `BasicReturn*` replies back to
/// our caller, rewriting each reply's UUID to the caller's. Runs as its own task so the serve recv
/// loop keeps going. Applies the trust-based depth budget and the visited-set (keyed by identity
/// public key) so the fan-out always terminates and never loops back on itself.
async fn discovery_forward(
  executor: std::sync::Arc<executor::Executor>,
  local_config: std::sync::Arc<config::Config>,
  incoming: executor::ProgramData,
  forward_uuid: Option<[u8; 16]>,
  caller_addr: std::net::SocketAddr,
  sock: std::sync::Arc<tokio::net::UdpSocket>,
  port: u16,
) {
  // We can only forward if we have a signed identity to sign the sub-request `source` with.
  let our_identity = match executor.identity_data() { Some(id) => id, None => return };
  let our_pubkey = executor.identity_pubkey();

  // UUID for our onward sub-requests: prefer the program-generated one (host::set_forward_uuid,
  // seeded from host::random in network-map.c); fall back to a fresh random one.
  let child_uuid = forward_uuid.unwrap_or_else(discovery::random_uuid16);

  // The path we hand our children: everything we were told, plus ourselves.
  let mut child_visited = incoming.visited.clone();
  child_visited.push(our_pubkey.clone());

  // De-duplicated forwarding targets: configured [[peer]]s + passively-observed neighbours.
  let mut targets: Vec<(std::net::SocketAddr, Option<Vec<u8>>)> = Vec::new();
  let mut seen_addrs: std::collections::HashSet<std::net::SocketAddr> = std::collections::HashSet::new();
  for peer in local_config.peer.iter() {
    if let Some(addr) = net_utils::resolve_peer_addr(peer, port).await {
      let pubkey = peer.expected_key_str()
        .and_then(|s| crypto_utils::public_key_to_ed25519_vk(s).ok())
        .map(|vk| vk.as_bytes().to_vec());
      if seen_addrs.insert(addr) { targets.push((addr, pubkey)); }
    }
  }
  for (addr, pubkey) in executor.observed_targets() {
    if seen_addrs.insert(addr) {
      targets.push((addr, if pubkey.is_empty() { None } else { Some(pubkey) }));
    }
  }

  for (peer_addr, peer_pubkey) in targets {
    // Loop prevention: skip ourselves and any peer already on the path (by identity pubkey).
    if let Some(pk) = &peer_pubkey {
      if *pk == our_pubkey { continue; }
      if incoming.visited.iter().any(|v| v == pk) { continue; }
    }
    let we_trust = peer_pubkey.as_deref().map(|pk| executor.trusts_pubkey(pk)).unwrap_or(false);
    let child_depth = discovery::child_depth_budget(incoming.depth_budget, we_trust);

    let sub = match executor::ProgramDataBuilder::new()
      .set_human_name(&incoming.human_name)
      .set_wasm_program_bytes(&incoming.wasm_program_bytes)
      .set_source(&our_identity)
      .set_request_context(child_uuid, child_depth, child_visited.clone())
      .build()
    {
      Ok(pd) => pd,
      Err(_) => continue,
    };
    let req = messages::NetworkMessage::ExecuteRequest { program_data: sub };
    let sub_bytes = match serde_bare::to_vec(&req) { Ok(b) => b, Err(_) => continue };

    tokio::spawn(relay_one_peer(sub_bytes, peer_addr, caller_addr, incoming.request_uuid, sock.clone()));
  }
}

/// Send one forwarded discovery sub-request to `peer_addr`, then relay every `BasicReturn*` reply back
/// to `caller_addr` on `sock`, rewriting the reply's UUID to `caller_uuid`. Ends when the subtree goes
/// quiet or the per-hop cap elapses.
async fn relay_one_peer(
  sub_bytes: Vec<u8>,
  peer_addr: std::net::SocketAddr,
  caller_addr: std::net::SocketAddr,
  caller_uuid: [u8; 16],
  sock: std::sync::Arc<tokio::net::UdpSocket>,
) {
  let bind: (std::net::IpAddr, u16) = if peer_addr.is_ipv4() {
    (std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
  } else {
    (std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
  };
  let relay = match tokio::net::UdpSocket::bind(bind).await { Ok(s) => s, Err(_) => return };
  if relay.send_to(&sub_bytes, peer_addr).await.is_err() { return; }

  let mut buf = [0u8; 64 * 1024];
  let start = std::time::Instant::now();
  let quiet = std::time::Duration::from_millis(RELAY_QUIET_TIMEOUT_MS);
  loop {
    if start.elapsed() >= std::time::Duration::from_millis(RELAY_MAX_MS) { break; }
    match tokio::time::timeout(quiet, relay.recv_from(&mut buf)).await {
      Ok(Ok((len, _from))) => {
        if let Ok(msg) = serde_bare::from_slice::<messages::NetworkMessage>(&buf[..len]) {
          let rewritten = match msg {
            messages::NetworkMessage::BasicReturnMap { from_pid, cbor_data, .. } =>
              Some(messages::NetworkMessage::BasicReturnMap { from_pid, request_uuid: caller_uuid, cbor_data }),
            messages::NetworkMessage::BasicReturnList { from_pid, cbor_data, .. } =>
              Some(messages::NetworkMessage::BasicReturnList { from_pid, request_uuid: caller_uuid, cbor_data }),
            _ => None,
          };
          if let Some(out) = rewritten {
            if let Ok(enc) = serde_bare::to_vec(&out) {
              let _ = sock.send_to(&enc, caller_addr).await;
            }
          }
        }
      }
      Ok(Err(_)) => break, // socket error
      Err(_) => break,     // quiet timeout: subtree has gone silent
    }
  }
}

#[derive(Clone)]
pub struct UdpSocketSender {
    socket: std::sync::Arc<tokio::net::UdpSocket>,
}

impl UdpSocketSender {
  pub fn new(socket: &std::sync::Arc<tokio::net::UdpSocket>) -> UdpSocketSender {
    UdpSocketSender {
      socket: socket.clone()
    }
  }
    pub async fn send_to(
        &self,
        buf: &[u8],
        addr: std::net::SocketAddr,
    ) -> std::io::Result<usize> {
        self.socket.send_to(buf, addr).await
    }

    pub fn poll_send_to(
        &self,
        cx: &mut core::task::Context<'_>,
        buf: &[u8],
        addr: std::net::SocketAddr,
    ) -> core::task::Poll<std::io::Result<usize>> {
        self.socket.poll_send_to(cx, buf, addr)
    }
}

