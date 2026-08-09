
use super::*;

use std::collections::HashMap;
use std::net::SocketAddr;

/// Shared collection point: for every responder socket address, the raw stdout bytes it sent back.
/// Because the discovery program emits its whole report in a single `host::print`, one datagram
/// per responder carries a complete report, so keying by source address cleanly separates nodes.
type ReportMap = std::sync::Arc<tokio::sync::Mutex<HashMap<SocketAddr, Vec<u8>>>>;

/// `weverywhere netmap` entry point: send the discovery program onto the fabric, gather the reports
/// nodes send back, and print a trust-annotated tree. See readme "Network discovery".
pub async fn netmap(
  args: &args::Args,
  program: Option<std::path::PathBuf>,
  local: bool,
  multicast_groups: args::MulticastAddressVec,
  port: u16,
) -> DynResult<()> {
  let (wasm_bytes, program_label) = resolve_discovery_program(program).await?;
  if crate::v_is_info() {
    tracing::info!("[ netmap ] discovery program: {}", program_label);
  }

  // Sign the discovery program with our identity, exactly like `run` does, so servers can decide
  // whether they trust us (which they then report back to us as the per-node trust marker).
  let local_config = config::Config::read_from_file(&args.config_path()).await.map_err(map_loc_err!())?;
  let source = config::IdentityData::generate_from_config(&local_config).await.map_err(map_loc_err!())?;

  let pd = executor::ProgramDataBuilder::new()
    .set_human_name(EMBEDDED_DISCOVERY_NAME.to_string())
    .set_wasm_program_bytes(&wasm_bytes)
    .set_source(&source)
    .build().map_err(map_loc_err!())?;

  let execute_req = messages::NetworkMessage::ExecuteRequest { program_data: pd };
  let execute_req_encoded = serde_bare::to_vec(&execute_req)?;

  let reports: ReportMap = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

  if local {
    collect_from_local_daemon(&execute_req_encoded, port, reports.clone()).await?;
  } else {
    collect_from_fabric(&execute_req_encoded, &multicast_groups, &local_config.peer, port, reports.clone()).await?;
  }

  let reports = reports.lock().await;
  print_network_graph(&source.human_name, &reports);

  Ok(())
}

/// The stem of the bundled discovery program (see example-programs/embedded.list + network-map.c).
const EMBEDDED_DISCOVERY_NAME: &str = "network-map";

/// On-disk location of the compiled discovery program, used only as a last-resort dev fallback when
/// nothing is embedded (e.g. zig was missing at build time).
fn default_program_path() -> std::path::PathBuf {
  std::path::PathBuf::from("target").join("example-programs").join(format!("{EMBEDDED_DISCOVERY_NAME}.wasm"))
}

/// Resolve which discovery program bytes to send, in precedence order:
///   1. an explicit `--program <FILE>` override (always wins);
///   2. the program embedded in this binary at build time (the normal, file-free path);
///   3. the compiled example on disk (dev convenience when nothing was embedded).
/// Returns the bytes plus a short human label describing where they came from.
async fn resolve_discovery_program(program: Option<std::path::PathBuf>) -> DynResult<(Vec<u8>, String)> {
  // 1. Explicit override.
  if let Some(path) = program {
    match tokio::fs::read(&path).await {
      Ok(bytes) => return Ok((bytes, format!("override file {}", path.display()))),
      Err(e) => return Err(format!("Could not read --program {:?} ({})", path, e).into()),
    }
  }

  // 2. Embedded bytes — the whole point: a moved binary needs no external .wasm.
  if let Some(bytes) = crate::embedded_programs::get(EMBEDDED_DISCOVERY_NAME) {
    return Ok((bytes.to_vec(), format!("embedded '{EMBEDDED_DISCOVERY_NAME}'")));
  }

  // 3. Dev fallback: the on-disk compiled example.
  let fallback = default_program_path();
  match tokio::fs::read(&fallback).await {
    Ok(bytes) => Ok((bytes, format!("on-disk {}", fallback.display()))),
    Err(e) => Err(format!(
      "No discovery program available: this binary has no embedded '{EMBEDDED_DISCOVERY_NAME}' and \
       {:?} could not be read ({}).\n\
       Build the binary with zig installed to embed it, run \
       `uv run scripts/compile-example-programs.py` to produce the on-disk copy, \
       or pass your own with `weverywhere netmap --program <FILE.wasm>`.",
      fallback, e
    ).into()),
  }
}

/// --local path: fire the discovery program at the daemon on 127.0.0.1 and collect its reply.
async fn collect_from_local_daemon(req: &[u8], port: u16, reports: ReportMap) -> DynResult<()> {
  let sock = tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await.map_err(map_loc_err!())?;
  sock.send_to(req, (std::net::Ipv4Addr::LOCALHOST, port)).await.map_err(map_loc_err!())?;
  collect_replies(&sock, reports).await
}

/// Default path: send the discovery program to every multicast group on every interface AND to every
/// statically-configured `[[peer]]`, each on its own socket/task, and merge the replies into the
/// shared report map. Peers are treated exactly like multicast targets - same request, same reply
/// collection - so nodes multicast can't reach still appear on the map.
async fn collect_from_fabric(req: &[u8], multicast_groups: &args::MulticastAddressVec, peers: &[config::PeerMetadata], port: u16, reports: ReportMap) -> DynResult<()> {
  let mut tasks = tokio::task::JoinSet::new();

  for peer in peers.iter() {
    let req = req.to_vec();
    let peer = peer.clone();
    let reports = reports.clone();
    tasks.spawn(async move {
      if let Err(e) = netmap_one_peer(&req, &peer, port, reports).await {
        tracing::warn!("[ netmap ] Error sending to peer [{}]: {:?}", peer.label(), e);
      }
    });
  }

  for (iface_idx, iface_name, iface_addrs) in net_utils::get_interfaces().into_iter() {
    for multicast_addr in multicast_groups.iter() {
      if iface_addrs.len() < 1 {
        // No addresses => treat as no network connection and skip the interface.
        continue;
      }
      let req = req.to_vec();
      let iface_name = iface_name.clone();
      let iface_addrs = iface_addrs.clone();
      let multicast_addr = multicast_addr.clone();
      let reports = reports.clone();
      tasks.spawn(async move {
        if let Err(e) = fabric_one_iface(&req, iface_idx, &iface_addrs, &multicast_addr, port, reports).await {
          if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::AddrInUse {
              return; // Expected for some ipv6 link-local addresses; not worth warning about.
            }
          }
          tracing::warn!("[ netmap ] Error on iface {:?} group {:?} port {}: {:?}", iface_name, multicast_addr, port, e);
        }
      });
    }
  }
  tasks.join_all().await;
  Ok(())
}

/// Send the discovery program to one multicast group on one interface, then collect replies that
/// arrive back on the same ephemeral socket.
async fn fabric_one_iface(req: &[u8], iface_idx: u32, iface_addrs: &Vec<std::net::IpAddr>, multicast_group: &std::net::IpAddr, port: u16, reports: ReportMap) -> DynResult<()> {
  let empty_bind_addr_port = if multicast_group.is_ipv4() {
    (std::net::IpAddr::V4(core::net::Ipv4Addr::new(0, 0, 0, 0)), 0)
  } else {
    (std::net::IpAddr::V6(core::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)), 0)
  };

  let sock = tokio::net::UdpSocket::bind(empty_bind_addr_port).await.map_err(map_loc_err!())?;

  if multicast_group.is_ipv4() {
    sock.set_multicast_loop_v4(true).map_err(map_loc_err!())?;
    sock.set_multicast_ttl_v4(4).map_err(map_loc_err!())?; // A few hops; matches `run`/`serve`.
  } else {
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

  sock.send_to(req, (*multicast_group, port)).await.map_err(map_loc_err!())?;

  collect_replies(&sock, reports).await
}

/// Unicast the discovery program to one configured `[[peer]]`, then collect replies that arrive back
/// on the same ephemeral socket. Mirrors `fabric_one_iface` but for a static peer: the target address
/// is chosen in preference order (hostname, then ipv6, then ipv4) and the socket family matches it.
async fn netmap_one_peer(req: &[u8], peer: &config::PeerMetadata, port: u16, reports: ReportMap) -> DynResult<()> {
  let target = match net_utils::resolve_peer_addr(peer, port).await {
    Some(t) => t,
    None => return Err(format!("no resolvable address for peer [{}]", peer.label()).into()),
  };

  let bind_addr = if target.is_ipv4() {
    (std::net::IpAddr::V4(core::net::Ipv4Addr::UNSPECIFIED), 0)
  } else {
    (std::net::IpAddr::V6(core::net::Ipv6Addr::UNSPECIFIED), 0)
  };
  let sock = tokio::net::UdpSocket::bind(bind_addr).await.map_err(map_loc_err!())?;

  sock.send_to(req, target).await.map_err(map_loc_err!())?;

  collect_replies(&sock, reports).await
}

/// Read datagrams for a short window, accumulating any forwarded stdout bytes per responder. The
/// window auto-extends whenever we actually hear something, so slow responders still get counted.
async fn collect_replies(sock: &tokio::net::UdpSocket, reports: ReportMap) -> DynResult<()> {
  let td = tokio::time::Duration::from_millis(150);
  let mut buf = [0u8; 64 * 1024];
  let mut remaining_checks: usize = 20; // ~3s baseline before any replies extend it

  while remaining_checks > 0 {
    remaining_checks -= 1;
    match tokio::time::timeout(td, sock.recv_from(&mut buf)).await {
      Ok(Ok((len, addr))) => {
        #[allow(unreachable_patterns)]
        match serde_bare::from_slice::<messages::NetworkMessage>(&buf[..len]) {
          Ok(messages::NetworkMessage::BasicInsecureProgramStdout { stdout_data, .. }) => {
            remaining_checks += 6; // heard something; keep listening a little longer
            let mut map = reports.lock().await;
            // One datagram carries a node's whole report. The same node can answer on
            // both the multicast and the [[peer]] path (its reply source is the same
            // server addr:port either way), so keep the first complete report per
            // responder and ignore repeats rather than concatenating duplicates.
            map.entry(addr).or_insert_with(|| stdout_data.to_vec());
          }
          Ok(_other) => { /* program-exit and friends aren't part of the map */ }
          Err(e) => {
            tracing::warn!("[ netmap ] parse error from {:?}: {e}", addr);
          }
        }
      }
      Ok(Err(e)) => tracing::warn!("[ netmap ] socket error: {e}"),
      Err(_) => { /* 150ms tick with no data */ }
    }
  }
  Ok(())
}

/// One responding node's parsed report.
#[derive(Default)]
struct NodeReport {
  hostname: Option<String>,
  trusts_you: bool,
  peers: Vec<PeerLine>,
}

/// One neighbour line inside a node's report.
struct PeerLine {
  name: String,
  addr: String,
  trusted_by_node: bool,
  pubkey_hex: String,
}

/// Parse the tab-separated lines the discovery program prints (see example-programs/network-map.c).
///   NODE\t<hostname>\t<trusts_you 0/1>
///   PEER\t<name>\t<addr>\t<trusted 0/1>\t<pubkey_hex>
fn parse_node(bytes: &[u8]) -> NodeReport {
  let text = String::from_utf8_lossy(bytes);
  let mut node = NodeReport::default();
  for line in text.lines() {
    let mut f = line.split('\t');
    match f.next() {
      Some("NODE") => {
        node.hostname = f.next().map(|s| s.to_string());
        node.trusts_you = f.next().map(|s| s.trim() == "1").unwrap_or(false);
      }
      Some("PEER") => {
        node.peers.push(PeerLine {
          name: f.next().unwrap_or("").to_string(),
          addr: f.next().unwrap_or("").to_string(),
          trusted_by_node: f.next().map(|s| s == "1").unwrap_or(false),
          pubkey_hex: f.next().unwrap_or("").to_string(),
        });
      }
      _ => { /* ignore blank / unknown lines for forward-compatibility */ }
    }
  }
  node
}

/// Render the collected reports as a tree rooted at "you", annotating each node with whether it
/// trusts you and each neighbour with whether that node trusts the neighbour.
fn print_network_graph(you: &str, reports: &HashMap<SocketAddr, Vec<u8>>) {
  println!();
  println!("weverywhere network map");
  println!("  you = {}", you);
  println!("  legend:  <3 = this host trusts you    x = this host does NOT trust you");
  println!();

  if reports.is_empty() {
    println!("(no servers responded)");
    println!("  - is a daemon running and reachable? try:  weverywhere netmap --local");
    println!("  - was the discovery program built?         uv run scripts/compile-example-programs.py");
    println!();
    return;
  }

  println!("(you) {}", you);

  // Stable output ordering regardless of arrival order.
  let mut entries: Vec<(&SocketAddr, &Vec<u8>)> = reports.iter().collect();
  entries.sort_by_key(|(addr, _)| addr.to_string());

  let node_count = entries.len();
  for (i, (addr, bytes)) in entries.into_iter().enumerate() {
    let node = parse_node(bytes);
    let is_last_node = i + 1 == node_count;
    let (branch, child_indent) = if is_last_node { ("`--", "    ") } else { ("|--", "|   ") };

    let host = node.hostname.unwrap_or_else(|| "<unknown>".to_string());
    let trust_mark = if node.trusts_you { "<3" } else { "x" };
    println!("{} {} @ {}   [{}]", branch, host, addr, trust_mark);

    if node.peers.is_empty() {
      println!("{}`-- (no peers observed yet)", child_indent);
      continue;
    }

    let peer_count = node.peers.len();
    for (j, peer) in node.peers.iter().enumerate() {
      let is_last_peer = j + 1 == peer_count;
      let peer_branch = if is_last_peer { "`--" } else { "|--" };
      let trust = if peer.trusted_by_node { "trusted" } else { "untrusted" };
      let name = if peer.name.is_empty() { "<anon>" } else { peer.name.as_str() };
      let short_key: String = peer.pubkey_hex.chars().take(8).collect();
      println!("{}{} peer: {} @ {}  [{}]  key:{}", child_indent, peer_branch, name, peer.addr, trust, short_key);
    }
  }
  println!();
}
