


pub fn get_interfaces() -> Vec<(u32, String)> {
    use getifs::Interface;
    getifs::interfaces()
        .into_iter()
        .flat_map(|tiny_vec| tiny_vec.into_iter())
        .map(|iface: Interface| (iface.index(), iface.name().to_string() ))
        .collect()
}
