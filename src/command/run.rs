
use super::*;


pub async fn run(args: &args::Args, file_path: &std::path::PathBuf, fabric: bool, multicast_groups: args::MulticastAddressVec, port: u16, arg_list: Vec<String>, arg_map: Vec<(String, String)>) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

  // Step 1: Read the executable material & form an exeute request object, sign it, and transmit.
  let wasm_bytes = tokio::fs::read(file_path).await.map_err(map_loc_err!())?;

  let local_config = config::Config::read_from_file(&args.config_path()).await.map_err(map_loc_err!())?;

  let source = config::IdentityData::generate_from_config(&local_config).await.map_err(map_loc_err!())?;

  let pd = executor::ProgramDataBuilder::new()
    .set_human_name(
      file_path.file_name().map(|fn_osstr| fn_osstr.to_string_lossy().to_string() ).unwrap_or_else(|| "UNSET_NAME".to_string() )
    )
    .set_wasm_program_bytes(&wasm_bytes)
    .set_source(&source)
    .set_args(arg_list, arg_map)
    .build().map_err(map_loc_err!())?;

  let execute_req = messages::NetworkMessage::ExecuteRequest {
    program_data: pd.clone(),
  };
  let execute_req_encoded = serde_bare::to_vec(&execute_req)?;

  // Step 2a (default): talk to the local daemon on this machine. The daemon binds 0.0.0.0:port,
  // so a unicast to the loopback address reaches it without touching the LAN. This is the
  // client's default on every platform; --fabric opts into the multicast broadcast below.
  if !fabric {
    return send_to_local_daemon(&execute_req_encoded, port).await;
  }

  // Step 2b (--fabric): transmit to all multicast groups on all interfaces, AND to
  // every statically-configured [[peer]] (unicast). Peers are treated just like the
  // multicast targets - sent the same request, replies collected the same way - so
  // nodes that multicast can't reach are still covered.
  let mut tasks = tokio::task::JoinSet::new();

  for peer in local_config.peer.iter() {
    let execute_req_encoded = execute_req_encoded.clone();
    let peer = peer.clone();
    tasks.spawn(async move {
      if let Err(e) = run_one_peer(&execute_req_encoded, &peer, port).await {
        tracing::warn!("[ run ] Error sending to peer [{}]: {:?}", peer.label(), e);
      }
    });
  }

  for (iface_idx, iface_name, iface_addrs) in net_utils::get_interfaces().into_iter() {
    for multicast_addr in multicast_groups.iter() {
      if iface_addrs.len() < 1 {
        // We assume 0 addresses means no network connection, so we skip the interface entirely.
        continue;
      }
      // Clone locals to appease async gods; TODO let's have better than Go's better memory management
      let file_path = file_path.clone();
      let iface_idx = iface_idx.clone();
      let iface_name = iface_name.clone();
      let iface_addrs = iface_addrs.clone();
      let multicast_addr = multicast_addr.clone();
      let pd = pd.clone();
      let execute_req_encoded = execute_req_encoded.clone();
      tasks.spawn(async move {
        if let Err(e) = run_one_iface(&execute_req_encoded, &pd, iface_idx, &iface_name, &iface_addrs, &multicast_addr, port).await {
          tracing::warn!("[ serve_iface ] Error serving {:?} addr {:?} port {}: {:?}", iface_name, multicast_addr, port, e);
        }
      });
    }
  }

  tasks.join_all().await;

  Ok(())
}

/// Fire-and-forget broadcast of a program onto the whole fabric (multicast on every interface +
/// unicast to every configured `[[peer]]`), signed with `source`. Unlike [`run`] this does not wait
/// for replies - it's the send side of `host::replicate`, used by self-propagating/role-shifting
/// programs (e.g. the chat "ui" role fanning a "deliver" copy out to peers). The copy carries the
/// given args and an empty discovery context (depth 0 / no visited), so recipients run it but don't
/// themselves recurse unless their own logic calls replicate again.
/// A UDP socket whose multicast egress interface is pinned to the NIC that owns `iface_addr` (via
/// IP_MULTICAST_IF). Bound to an ephemeral port; TTL 4 and loopback on so a co-located listener still
/// gets our own copy. tokio's UdpSocket can't set IP_MULTICAST_IF, so we go through socket2.
fn new_multicast_sender_v4(iface_addr: std::net::Ipv4Addr) -> DynResult<tokio::net::UdpSocket> {
  use socket2::{Domain, Protocol, Socket, Type};
  let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(map_loc_err!())?;
  sock.set_multicast_if_v4(&iface_addr).map_err(map_loc_err!())?;
  sock.set_multicast_ttl_v4(4).map_err(map_loc_err!())?;
  let _ = sock.set_multicast_loop_v4(true);
  sock.set_nonblocking(true).map_err(map_loc_err!())?;
  sock.bind(&std::net::SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0)).into()).map_err(map_loc_err!())?;
  Ok(tokio::net::UdpSocket::from_std(sock.into()).map_err(map_loc_err!())?)
}

/// IPv6 counterpart of [`new_multicast_sender_v4`], pinning the egress interface by index.
fn new_multicast_sender_v6(iface_idx: u32) -> DynResult<tokio::net::UdpSocket> {
  use socket2::{Domain, Protocol, Socket, Type};
  let sock = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)).map_err(map_loc_err!())?;
  sock.set_multicast_if_v6(iface_idx).map_err(map_loc_err!())?;
  let _ = sock.set_multicast_loop_v6(true);
  sock.set_nonblocking(true).map_err(map_loc_err!())?;
  sock.bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0)).into()).map_err(map_loc_err!())?;
  Ok(tokio::net::UdpSocket::from_std(sock.into()).map_err(map_loc_err!())?)
}

pub async fn broadcast_program_to_fabric(
  wasm_bytes: &[u8],
  source: &config::IdentityData,
  human_name: &str,
  arg_list: Vec<String>,
  arg_map: Vec<(String, String)>,
  multicast_groups: &[std::net::IpAddr],
  port: u16,
  peers: &[config::PeerMetadata],
) -> DynResult<()> {
  let pd = executor::ProgramDataBuilder::new()
    .set_human_name(human_name)
    .set_wasm_program_bytes(wasm_bytes)
    .set_source(source)
    .set_args(arg_list, arg_map)
    .build()?;
  let bytes = serde_bare::to_vec(&messages::NetworkMessage::ExecuteRequest { program_data: pd })?;

  // Multicast each group out EVERY interface, pinning the egress with IP_MULTICAST_IF. A plain
  // 0.0.0.0:0 send picks only the default-route interface, so hosts that straddle several segments (a
  // physical LAN plus a VM bridge, say) deliver multicast to just one of them - the others never see
  // it. Sending per interface reaches all segments; where copies overlap on a shared link, receivers
  // collapse them via the (pubkey, id) dedup. Losing a copy is fine - senders/operators retry.
  let interfaces = net_utils::get_interfaces();
  let mut sent_any = false;
  for group in multicast_groups.iter() {
    for (idx, name, addrs) in interfaces.iter() {
      // Send once per interface, pinning egress to that NIC. v4 selects the NIC by one of its
      // addresses; v6 by interface index. An interface with no address of the group's family can't
      // carry it, so it's skipped. Per-interface errors (down link, no multicast route) are logged
      // but never abort the fan-out - reaching the other interfaces is what matters.
      let sock = match group {
        std::net::IpAddr::V4(_) => match addrs.iter().find_map(|a| match a { std::net::IpAddr::V4(v4) => Some(*v4), _ => None }) {
          Some(iface_v4) => new_multicast_sender_v4(iface_v4),
          None => continue,
        },
        std::net::IpAddr::V6(_) => {
          if !addrs.iter().any(|a| a.is_ipv6()) { continue; }
          new_multicast_sender_v6(*idx)
        }
      };
      let sock = match sock {
        Ok(s) => s,
        Err(e) => { if crate::v_is_info() { tracing::warn!("[ multicast ] {} on {}: socket setup failed: {:?}", group, name, e); } continue; }
      };
      match sock.send_to(&bytes, (*group, port)).await {
        Ok(n) => { sent_any = true; if crate::v_is_info() { tracing::warn!("[ multicast ] {} bytes -> {} via {}", n, group, name); } }
        Err(e) => { if crate::v_is_info() { tracing::warn!("[ multicast ] {} via {} failed: {:?}", group, name, e); } }
      }
    }
  }
  if !sent_any {
    tracing::warn!("[ multicast ] sent on NO interfaces (groups {:?}) - fabric got only unicast peers", multicast_groups);
  }
  // Unicast to every configured peer as well (covers hosts multicast can't reach).
  for peer in peers.iter() {
    if let Some(addr) = net_utils::resolve_peer_addr(peer, port).await {
      let bind = if addr.is_ipv4() {
        (std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
      } else {
        (std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
      };
      if let Ok(sock) = tokio::net::UdpSocket::bind(bind).await {
        let _ = sock.send_to(&bytes, addr).await;
      }
    }
  }
  Ok(())
}

pub async fn run_one_iface(ex_req_bytes: &[u8], pd: &executor::ProgramData, iface_idx: u32, iface_name: &str, iface_addrs: &Vec<std::net::IpAddr>, multicast_group: &std::net::IpAddr, port: u16) -> DynResult<()> {

  if crate::v_is_info() {
    tracing::warn!("Sending {} bytes to {:?} port {} on iface {} ({:?})", ex_req_bytes.len(), multicast_group, port, iface_name, iface_addrs);
  }

  let empty_bind_addr_port = if multicast_group.is_ipv4() {
    (std::net::IpAddr::V4(core::net::Ipv4Addr::new(0, 0, 0, 0)), 0 )
  }
  else {
    (std::net::IpAddr::V6(core::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)), 0 )
  };

  let sock = tokio::net::UdpSocket::bind(empty_bind_addr_port).await.map_err(map_loc_err!())?;

  if multicast_group.is_ipv4() {
    sock.set_multicast_loop_v4(true).map_err(map_loc_err!())?;
    sock.set_multicast_ttl_v4(4).map_err(map_loc_err!())?; // How many hops multicast can live for - default is just the immediate LAN we are attached to. TODO configure me from /etc/weveryware.toml l8ter
  }
  else {
    sock.set_multicast_loop_v6(true).map_err(map_loc_err!())?;
  }

  match multicast_group {
    std::net::IpAddr::V4(multicast_group) => {
      for iface_addr in iface_addrs.iter() {
        if let std::net::IpAddr::V4(iface_addr_v4) = iface_addr {
          sock.join_multicast_v4(*multicast_group, *iface_addr_v4).map_err(map_loc_err!())?;
        }
      }
    }
    std::net::IpAddr::V6(multicast_group) => {
      sock.join_multicast_v6(multicast_group, iface_idx).map_err(map_loc_err!())?;
    }
  }

  // sock.connect( (*multicast_group, port) ).await.map_err(map_loc_err!())?;
  // Sized to a full UDP datagram so large forwarded stdout payloads aren't truncated.
  let mut buf = [0; 64*1024];

  let len = sock.send_to(&ex_req_bytes, (*multicast_group, port)).await.map_err(map_loc_err!())?;
  tracing::warn!("{:?} bytes sent", len);

  let td = tokio::time::Duration::from_millis(100);

  let mut remaining_100ms_checks: usize = 24;

  while remaining_100ms_checks > 0 {
    remaining_100ms_checks -= 1;
    // Only wait up to 100ms for replies;
    match tokio::time::timeout(td, sock.recv(&mut buf)).await {
      Ok(Ok(len)) => {
        if crate::v_is_everything() {
          tracing::warn!("{:?} bytes received from {:?} => {:?}", len, multicast_group, &buf[0..len]);
        }
        #[allow(unreachable_patterns)]
        match serde_bare::from_slice::<messages::NetworkMessage>(&buf[..len]) {
          Ok(network_message) => {
            remaining_100ms_checks += 10; // if we rx a message, allow another second of waiting.
            match network_message {
              messages::NetworkMessage::BasicInsecureProgramStdout { from_pid, stdout_data } => {
                if let Ok(stdout_string) = str::from_utf8(&stdout_data) {
                  tracing::warn!("[{}] {}", from_pid, stdout_string);
                }
                else {
                  tracing::warn!("[{}:binary] {:?}", from_pid, stdout_data);
                }
              }
              messages::NetworkMessage::BasicInsecureProgramExit { from_pid, exit_code } => {
                tracing::warn!("pid {} exited with code {}", from_pid, exit_code);
              }
              unused => {
                tracing::warn!("Got unexpected network message: {:?}", unused);
              }
            }
          }
          Err(e) => {
            tracing::warn!("Parsing NetworkMessage error: {e}");
          }
        }
      }
      Ok(Err(e)) => {
        // The socket operation itself failed
        tracing::warn!("Socket error: {e}");
      }
      Err(_) => {
        // The timeout expired (no data within 100ms)
        // tracing::warn!("Timed out");
      }
    }
  }

  Ok(())
}

/// Unicast an encoded execute request to one configured `[[peer]]` and print its replies, exactly
/// like the local-daemon path. The peer's address is chosen in preference order (hostname, then
/// ipv6, then ipv4); the reply socket is bound to the matching address family.
pub async fn run_one_peer(ex_req_bytes: &[u8], peer: &config::PeerMetadata, port: u16) -> DynResult<()> {
  let target = match net_utils::resolve_peer_addr(peer, port).await {
    Some(t) => t,
    None => return Err(format!("no resolvable address for peer [{}]", peer.label()).into()),
  };

  let bind_addr = if target.is_ipv4() {
    (std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0)
  } else {
    (std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0)
  };
  let sock = tokio::net::UdpSocket::bind(bind_addr).await.map_err(map_loc_err!())?;

  let len = sock.send_to(ex_req_bytes, target).await.map_err(map_loc_err!())?;
  if crate::v_is_info() {
    tracing::warn!("Sent {} bytes to peer [{}] at {}", len, peer.label(), target);
  }

  read_daemon_replies(&sock).await
}

/// Default client path: send an encoded execute request to the local daemon over loopback, then
/// print replies for a short window. The daemon binds 0.0.0.0:port, so a unicast to 127.0.0.1
/// reaches it on every platform without going out to the LAN.
pub async fn send_to_local_daemon(ex_req_bytes: &[u8], port: u16) -> DynResult<()> {
  let sock = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await.map_err(map_loc_err!())?;
  let len = sock.send_to(ex_req_bytes, (std::net::Ipv4Addr::LOCALHOST, port)).await.map_err(map_loc_err!())?;
  tracing::warn!("{} bytes sent to local daemon 127.0.0.1:{}", len, port);
  read_daemon_replies(&sock).await
}

/// Read and print daemon replies (forwarded stdout + exit codes) for up to a short window.
async fn read_daemon_replies(sock: &tokio::net::UdpSocket) -> DynResult<()> {
  let td = tokio::time::Duration::from_millis(100);
  let mut buf = [0u8; 16 * 1024];
  let mut remaining_100ms_checks: usize = 24;
  while remaining_100ms_checks > 0 {
    remaining_100ms_checks -= 1;
    match tokio::time::timeout(td, sock.recv(&mut buf)).await {
      Ok(Ok(len)) => {
        #[allow(unreachable_patterns)]
        match serde_bare::from_slice::<messages::NetworkMessage>(&buf[..len]) {
          Ok(network_message) => {
            remaining_100ms_checks += 10; // got a reply: allow another ~second of waiting
            match network_message {
              messages::NetworkMessage::BasicInsecureProgramStdout { from_pid, stdout_data } => {
                if let Ok(stdout_string) = str::from_utf8(&stdout_data) {
                  tracing::warn!("[{}] {}", from_pid, stdout_string);
                }
                else {
                  tracing::warn!("[{}:binary] {:?}", from_pid, stdout_data);
                }
              }
              messages::NetworkMessage::BasicInsecureProgramExit { from_pid, exit_code } => {
                tracing::warn!("pid {} exited with code {}", from_pid, exit_code);
              }
              unused => {
                tracing::warn!("Got unexpected network message: {:?}", unused);
              }
            }
          }
          Err(e) => {
            tracing::warn!("Parsing NetworkMessage error: {e}");
          }
        }
      }
      Ok(Err(e)) => {
        tracing::warn!("Socket error: {e}");
      }
      Err(_) => { /* 100ms timeout, no data */ }
    }
  }
  Ok(())
}
