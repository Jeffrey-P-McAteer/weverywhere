



#[cfg(unix)]
pub fn get_interfaces() -> Vec<(u32, String)> {
    get_interfaces_unix()
}

#[cfg(windows)]
pub fn get_interfaces() -> Vec<(u32, String)> {
    get_interfaces_windows()
}




#[cfg(unix)]
fn get_interfaces_unix() -> Vec<(u32, String)> {
    use std::ffi::CString;
    use libc::if_nametoindex;

    let mut result = Vec::new();
    let ifaces = get_if_addrs::get_if_addrs().unwrap();

    let mut seen = std::collections::HashSet::new();
    for iface in ifaces {
        if seen.insert(iface.name.clone()) {
            let c_name = CString::new(iface.name.clone()).unwrap();
            let index = unsafe { if_nametoindex(c_name.as_ptr()) };
            if index != 0 {
                result.push((index, iface.name));
            }
        }
    }
    result
}


#[cfg(windows)]
fn get_interfaces_windows() -> Vec<(u32, String)> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_PREFIX, AF_UNSPEC, IP_ADAPTER_ADDRESSES_LH
    };

    let mut result = Vec::new();
    unsafe {
        let mut buf = vec![0u8; 15000];
        let p: *mut IP_ADAPTER_ADDRESSES_LH = buf.as_mut_ptr() as _;
        let _ = GetAdaptersAddresses(AF_UNSPEC.0 as _, GAA_FLAG_INCLUDE_PREFIX, std::ptr::null_mut(), p, &mut 15000);
        let mut current = p;
        while !current.is_null() {
            let name = std::ffi::CStr::from_ptr((*current).AdapterName).to_string_lossy().into_owned();
            let index = (*current).IfIndex; // use Ipv6IfIndex for IPv6
            result.push((index, name));
            current = (*current).Next;
        }
    }
    result
}
