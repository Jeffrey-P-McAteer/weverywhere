
use super::*;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

/// `weverywhere netmap` entry point: send the discovery program onto the fabric (multicast + every
/// configured [[peer]]), collect the signed per-node records that relay back for a fixed window, and
/// print a trust-annotated tree. Each node returns a signed attestation binding its hostname to its
/// identity key, so the caller can prove every entry represents only itself. See readme "Network
/// discovery" and `example-programs/network-map.c`.
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

  let local_config = config::Config::read_from_file(&args.config_path()).await.map_err(map_loc_err!())?;
  let source = config::IdentityData::generate_from_config(&local_config).await.map_err(map_loc_err!())?;

  // Our own identity pubkey: the tree root, and seeded into the visited-set so no node forwards back
  // to us. Best-effort - if we have no key we use an empty root marker.
  let our_pubkey = local_config.identity.read_public_key_ed25519_pem_file().await
    .map(|vk| vk.as_bytes().to_vec())
    .unwrap_or_default();

  // Keys we trust (config [[trusted]] + our own): first-hop peers whose pinned key is in here get the
  // deeper trusted forwarding budget.
  let mut trusted: HashSet<Vec<u8>> = HashSet::new();
  if !our_pubkey.is_empty() { trusted.insert(our_pubkey.clone()); }
  for t in local_config.trusted.iter() {
    if let Ok(vk) = crypto_utils::public_key_to_ed25519_vk(&t.key) {
      trusted.insert(vk.as_bytes().to_vec());
    }
  }

  // One request UUID for this whole invocation; every node's records relay back tagged with it.
  let request_uuid = discovery::random_uuid16();
  let visited_init: Vec<Vec<u8>> = if our_pubkey.is_empty() { Vec::new() } else { vec![our_pubkey.clone()] };

  // Collected records: raw (responder addr, cbor bytes) for replies carrying OUR request UUID.
  let collected: std::sync::Arc<tokio::sync::Mutex<Vec<(SocketAddr, Vec<u8>)>>> =
    std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

  // Build a per-target execute request (the depth budget differs by trust), encode + send.
  let make_request = |depth: u8| -> DynResult<Vec<u8>> {
    let pd = executor::ProgramDataBuilder::new()
      .set_human_name(EMBEDDED_DISCOVERY_NAME.to_string())
      .set_wasm_program_bytes(&wasm_bytes)
      .set_source(&source)
      .set_request_context(request_uuid, depth, visited_init.clone())
      .build().map_err(map_loc_err!())?;
    Ok(serde_bare::to_vec(&messages::NetworkMessage::ExecuteRequest { program_data: pd })?)
  };

  // Sockets we both send from and collect replies on (nodes reply to the address they were contacted
  // from). One per family; the v6 socket is best-effort.
  let sock_v4 = std::sync::Arc::new(tokio::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await.map_err(map_loc_err!())?);
  let sock_v6 = tokio::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).await.ok().map(std::sync::Arc::new);

  // Start collectors before sending so nothing is missed.
  let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(12);
  let mut collectors = tokio::task::JoinSet::new();
  {
    let sock = sock_v4.clone();
    let collected = collected.clone();
    collectors.spawn(async move { collect_until(&sock, request_uuid, deadline, collected).await; });
  }
  if let Some(sock6) = &sock_v6 {
    let sock = sock6.clone();
    let collected = collected.clone();
    collectors.spawn(async move { collect_until(&sock, request_uuid, deadline, collected).await; });
  }

  if local {
    // Only the local daemon: unicast to loopback with the untrusted budget (it's just us).
    let req = make_request(discovery::UNTRUSTED_FORWARD_DEPTH)?;
    let _ = sock_v4.send_to(&req, (std::net::Ipv4Addr::LOCALHOST, port)).await;
  } else {
    // Multicast to every group on every interface (untrusted budget - arbitrary hosts).
    let mcast_req = make_request(discovery::UNTRUSTED_FORWARD_DEPTH)?;
    for (_iface_idx, _iface_name, iface_addrs) in net_utils::get_interfaces().into_iter() {
      if iface_addrs.is_empty() { continue; }
      for group in multicast_groups.iter() {
        match group {
          std::net::IpAddr::V4(g) => {
            let _ = sock_v4.set_multicast_ttl_v4(4);
            let _ = sock_v4.send_to(&mcast_req, (*g, port)).await;
          }
          std::net::IpAddr::V6(g) => {
            if let Some(sock6) = &sock_v6 {
              let _ = sock6.send_to(&mcast_req, (*g, port)).await;
            }
          }
        }
      }
    }
    // Unicast to every configured [[peer]] with a per-peer trust-based depth budget.
    for peer in local_config.peer.iter() {
      if let Some(addr) = net_utils::resolve_peer_addr(peer, port).await {
        let peer_trusted = peer.expected_key_str()
          .and_then(|s| crypto_utils::public_key_to_ed25519_vk(s).ok())
          .map(|vk| trusted.contains(vk.as_bytes().as_slice()))
          .unwrap_or(false);
        let req = make_request(discovery::initial_depth_budget(peer_trusted))?;
        match addr {
          SocketAddr::V4(_) => { let _ = sock_v4.send_to(&req, addr).await; }
          SocketAddr::V6(_) => { if let Some(sock6) = &sock_v6 { let _ = sock6.send_to(&req, addr).await; } }
        }
      }
    }
  }

  // Wait out the 12s collection window.
  collectors.join_all().await;

  let replies = collected.lock().await;
  print_network_map(&source.human_name, &our_pubkey, &replies);
  Ok(())
}

/// The stem of the bundled discovery program (see example-programs/embedded.list + network-map.c).
const EMBEDDED_DISCOVERY_NAME: &str = "network-map";

/// On-disk location of the compiled discovery program, used only as a last-resort dev fallback.
fn default_program_path() -> std::path::PathBuf {
  std::path::PathBuf::from("target").join("example-programs").join(format!("{EMBEDDED_DISCOVERY_NAME}.wasm"))
}

/// Resolve which discovery program bytes to send: explicit `--program`, else the embedded program,
/// else the on-disk compiled example. Returns the bytes plus a short label describing the source.
async fn resolve_discovery_program(program: Option<std::path::PathBuf>) -> DynResult<(Vec<u8>, String)> {
  if let Some(path) = program {
    match tokio::fs::read(&path).await {
      Ok(bytes) => return Ok((bytes, format!("override file {}", path.display()))),
      Err(e) => return Err(format!("Could not read --program {:?} ({})", path, e).into()),
    }
  }
  if let Some(bytes) = crate::embedded_programs::get(EMBEDDED_DISCOVERY_NAME) {
    return Ok((bytes.to_vec(), format!("embedded '{EMBEDDED_DISCOVERY_NAME}'")));
  }
  let fallback = default_program_path();
  match tokio::fs::read(&fallback).await {
    Ok(bytes) => Ok((bytes, format!("on-disk {}", fallback.display()))),
    Err(e) => Err(format!(
      "No discovery program available: this binary has no embedded '{EMBEDDED_DISCOVERY_NAME}' and \
       {:?} could not be read ({}). Build with zig to embed it, run \
       `uv run scripts/compile-example-programs.py`, or pass `--program <FILE.wasm>`.",
      fallback, e
    ).into()),
  }
}

/// Read `BasicReturnMap` replies carrying `request_uuid` off `sock` until `deadline`, appending each
/// (responder addr, cbor bytes) to `collected`.
async fn collect_until(
  sock: &tokio::net::UdpSocket,
  request_uuid: [u8; 16],
  deadline: tokio::time::Instant,
  collected: std::sync::Arc<tokio::sync::Mutex<Vec<(SocketAddr, Vec<u8>)>>>,
) {
  let mut buf = [0u8; 64 * 1024];
  loop {
    let now = tokio::time::Instant::now();
    if now >= deadline { break; }
    match tokio::time::timeout(deadline - now, sock.recv_from(&mut buf)).await {
      Ok(Ok((len, from))) => {
        if let Ok(messages::NetworkMessage::BasicReturnMap { request_uuid: ru, cbor_data, .. }) =
          serde_bare::from_slice::<messages::NetworkMessage>(&buf[..len])
        {
          if ru == request_uuid {
            collected.lock().await.push((from, cbor_data));
          }
        }
      }
      Ok(Err(_)) => break,
      Err(_) => break, // deadline reached
    }
  }
}

/// One node in the assembled map.
struct Node {
  pubkey: Vec<u8>,
  hostname: String,
  parent: Vec<u8>,
  trusts_caller: bool,
  /// The node's OWN address as it reported it (record key 5). Preferred for display over `responder`,
  /// which for a relayed record is the intermediate daemon that forwarded it up, not the node itself.
  node_addr: Option<String>,
  responder: SocketAddr,
  sig_valid: bool,
  time_ok: bool,
}

/// Decode a per-node record CBOR map into its fields (attestation bytes + parent + trusts_caller +
/// the node's self-reported address).
fn parse_record(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>, bool, Option<String>)> {
  use serde_cbor::Value;
  let map = match serde_cbor::from_slice::<Value>(bytes).ok()? {
    Value::Map(m) => m,
    _ => return None,
  };
  let attestation = match map.get(&Value::Integer(discovery::record_keys::ATTESTATION)) {
    Some(Value::Bytes(b)) => b.clone(),
    _ => return None,
  };
  let parent = match map.get(&Value::Integer(discovery::record_keys::PARENT_PUBKEY)) {
    Some(Value::Bytes(b)) => b.clone(),
    _ => Vec::new(),
  };
  let trusts_caller = matches!(
    map.get(&Value::Integer(discovery::record_keys::TRUSTS_CALLER)),
    Some(Value::Integer(1))
  );
  // Accept the address as either a text or byte string; an empty value means "unknown".
  let node_addr = match map.get(&Value::Integer(discovery::record_keys::NODE_ADDR)) {
    Some(Value::Text(s)) if !s.is_empty() => Some(s.clone()),
    Some(Value::Bytes(b)) if !b.is_empty() => Some(String::from_utf8_lossy(b).into_owned()),
    _ => None,
  };
  Some((attestation, parent, trusts_caller, node_addr))
}

/// Verify + assemble the collected records into a tree and print it. Verifies each node's signature
/// (warning on failure) and that its timestamp is within the request window, dedups by identity
/// pubkey (never printing a node twice), and links children to parents by pubkey.
fn print_network_map(you: &str, our_pubkey: &[u8], replies: &[(SocketAddr, Vec<u8>)]) {
  let now = sys_utils::epoch_seconds_now_utc0();
  let mut nodes: HashMap<Vec<u8>, Node> = HashMap::new();

  for (responder, cbor) in replies {
    let (attestation, parent, trusts_caller, node_addr) = match parse_record(cbor) {
      Some(v) => v,
      None => continue,
    };
    match discovery::verify_attestation_cbor(&attestation) {
      Ok(vn) => {
        let time_ok = discovery::attestation_time_ok(vn.epoch_s, now);
        if !time_ok {
          eprintln!("[netmap] WARNING: {} ({}) attestation timestamp is outside the +/-30s window",
                    vn.hostname, responder);
        }
        // Dedup by identity pubkey: never re-print the same node.
        nodes.entry(vn.pubkey.clone()).or_insert(Node {
          pubkey: vn.pubkey,
          hostname: vn.hostname,
          parent,
          trusts_caller,
          node_addr,
          responder: *responder,
          sig_valid: true,
          time_ok,
        });
      }
      Err(e) => {
        eprintln!("[netmap] WARNING: invalid signature from {} - {} (node not trusted for identity)",
                  responder, e);
      }
    }
  }

  println!();
  println!("weverywhere network map");
  println!("  you = {}", you);
  println!("  legend:  <3 = node trusts you    x = node does NOT trust you    (!) = unverified/expired");
  println!();

  if nodes.is_empty() {
    println!("(no servers responded)");
    println!("  - is a daemon running and reachable? try:  weverywhere netmap --local");
    println!("  - are peers configured, and did the discovery program get built?");
    println!();
    return;
  }

  // children[parent_pubkey] = [child pubkeys], sorted for stable output.
  let mut children: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();
  for (pk, node) in nodes.iter() {
    children.entry(node.parent.clone()).or_default().push(pk.clone());
  }
  for v in children.values_mut() {
    v.sort_by(|a, b| {
      let na = nodes.get(a).map(|n| n.hostname.as_str()).unwrap_or("");
      let nb = nodes.get(b).map(|n| n.hostname.as_str()).unwrap_or("");
      na.cmp(nb)
    });
  }

  println!("(you) {}", you);
  // Roots are nodes whose parent is us (or whose parent we never heard from).
  let mut roots: Vec<Vec<u8>> = nodes.keys()
    .filter(|pk| {
      let parent = &nodes[*pk].parent;
      parent == our_pubkey || !nodes.contains_key(parent)
    })
    .cloned()
    .collect();
  roots.sort_by(|a, b| nodes[a].hostname.cmp(&nodes[b].hostname));

  let mut printed: HashSet<Vec<u8>> = HashSet::new();
  let root_count = roots.len();
  for (i, pk) in roots.iter().enumerate() {
    print_subtree(pk, &nodes, &children, "", i + 1 == root_count, &mut printed);
  }
  println!();
}

/// Recursively print one node and its children with box-drawing indentation, guarding against cycles
/// via `printed` (a node reached by two paths is only shown once).
fn print_subtree(
  pk: &[u8],
  nodes: &HashMap<Vec<u8>, Node>,
  children: &HashMap<Vec<u8>, Vec<Vec<u8>>>,
  prefix: &str,
  is_last: bool,
  printed: &mut HashSet<Vec<u8>>,
) {
  let node = match nodes.get(pk) { Some(n) => n, None => return };
  if !printed.insert(pk.to_vec()) {
    return; // already printed via another path
  }
  let branch = if is_last { "`--" } else { "|--" };
  let trust_mark = if node.trusts_caller { "<3" } else { "x" };
  let warn = if node.sig_valid && node.time_ok { "" } else { " (!)" };
  // Prefer the node's self-reported address; fall back to the datagram source (which for a relayed
  // record is the intermediate daemon, not this node).
  let addr = node.node_addr.clone().unwrap_or_else(|| node.responder.to_string());
  println!("{}{} {} @ {}   [{}]{}", prefix, branch, node.hostname, addr, trust_mark, warn);

  let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "|   " });
  if let Some(kids) = children.get(pk) {
    let kids: Vec<&Vec<u8>> = kids.iter().filter(|k| !printed.contains(*k)).collect();
    let n = kids.len();
    for (i, child) in kids.into_iter().enumerate() {
      print_subtree(child, nodes, children, &child_prefix, i + 1 == n, printed);
    }
  }
}
