


pub fn get_interfaces() -> Vec<(u32, String, Vec<std::net::IpAddr>)> {
    use getifs::Interface;
    getifs::interfaces()
        .into_iter()
        .flat_map(|tiny_vec| tiny_vec.into_iter())
        .map(|iface: Interface| (iface.index(), iface.name().to_string(), getifs_addrs_to_first_IpAddr( iface.addrs().map(|small_vec| small_vec.to_vec()) ) ))
        .collect()
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
