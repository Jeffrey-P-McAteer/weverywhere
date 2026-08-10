


pub fn get_interfaces() -> Vec<(u32, String, Vec<std::net::IpAddr>)> {
    use getifs::Interface;
    getifs::interfaces()
        .into_iter()
        .flat_map(|tiny_vec| tiny_vec.into_iter())
        .map(|iface: Interface| (iface.index(), iface.name().to_string(), getifs_addrs_to_first_IpAddr( iface.addrs().map(|small_vec| small_vec.to_vec()) ) ))
        .collect()
}

/// Resolve a statically-configured `[[peer]]` to a single socket address to send to,
/// honouring its preference order: hostname, then ipv6, then ipv4 (see
/// [`crate::config::PeerMetadata::connect_hosts`]). Each candidate is resolved via the
/// socket layer (DNS for hostnames, a parse for literal IPs); the first candidate that
/// resolves to at least one address wins. Returns `None` if nothing resolves.
pub async fn resolve_peer_addr(peer: &crate::config::PeerMetadata, port: u16) -> Option<std::net::SocketAddr> {
    for host in peer.connect_hosts() {
        match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.next() {
                    return Some(addr);
                }
            }
            Err(e) => {
                if crate::v_is_info() {
                    tracing::info!("[ resolve_peer_addr ] {host:?} did not resolve: {e}");
                }
            }
        }
    }
    None
}

/// Pick THIS host's own IP address most likely reachable on the same network as `peer` (the caller
/// that just contacted us): among our local interface addresses of the same family, the one sharing
/// the longest leading-bit prefix with `peer`. Used by discovery so each node can report its OWN
/// address instead of the relay it was reached through (see `src/command/serve.rs` relay path).
/// Loopback interface addresses are only considered when `peer` itself is loopback (the `--local`
/// case). Returns `None` if we have no same-family interface address.
pub fn local_addr_facing(peer: std::net::IpAddr) -> Option<std::net::IpAddr> {
  let peer_is_loopback = peer.is_loopback();
  let mut best: Option<(u32, std::net::IpAddr)> = None;
  for (_idx, _name, addrs) in get_interfaces() {
    for addr in addrs {
      let same_family = matches!(
        (addr, peer),
        (std::net::IpAddr::V4(_), std::net::IpAddr::V4(_)) | (std::net::IpAddr::V6(_), std::net::IpAddr::V6(_))
      );
      if !same_family || addr.is_unspecified() { continue; }
      if addr.is_loopback() && !peer_is_loopback { continue; }
      let bits = common_prefix_bits(addr, peer);
      if best.map_or(true, |(best_bits, _)| bits > best_bits) {
        best = Some((bits, addr));
      }
    }
  }
  best.map(|(_, addr)| addr)
}

/// Number of leading bits two same-family IP addresses share (a proxy for "same subnet" without
/// needing netmasks). Mismatched families share none.
fn common_prefix_bits(a: std::net::IpAddr, b: std::net::IpAddr) -> u32 {
  match (a, b) {
    (std::net::IpAddr::V4(a), std::net::IpAddr::V4(b)) => (u32::from(a) ^ u32::from(b)).leading_zeros(),
    (std::net::IpAddr::V6(a), std::net::IpAddr::V6(b)) => (u128::from(a) ^ u128::from(b)).leading_zeros(),
    _ => 0,
  }
}

#[allow(non_snake_case)]
pub fn getifs_addrs_to_first_IpAddr(addrs: std::io::Result<Vec<getifs::IfNet>>) -> Vec<std::net::IpAddr> {
    let mut all_addrs = vec![];
    if let Ok(addrs) = addrs {
        for addr in addrs.iter() {
            all_addrs.push(addr.addr().into());
        }
    }
    all_addrs
}
