


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
