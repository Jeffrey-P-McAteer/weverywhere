use super::*;


#[allow(unreachable_code)]
pub async fn serve(multicast_group: args::MulticastAddressVec, port: u16) -> DynResult<()> {
  let mut tasks = tokio::task::JoinSet::new();
  for (iface_idx, iface_name, iface_addrs) in net_utils::get_interfaces().into_iter() {
    for multicast_addr in multicast_group.iter() {
      if iface_addrs.len() < 1 {
        // We assume 0 addresses means no network connection, so we skip the interface entirely.
        continue;
      }
      // Clone locals to appease async gods
      let iface_idx = iface_idx.clone();
      let iface_name = iface_name.clone();
      let iface_addrs = iface_addrs.clone();
      let multicast_addr = multicast_addr.clone();
      tasks.spawn(async move {
        if let Err(e) = serve_iface(iface_idx, &iface_name, &iface_addrs, &multicast_addr, port).await {
          if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            if io_err.kind() == std::io::ErrorKind::AddrInUse {
              return; // Don't bother warning, we see this w/ ipv6 link-local addresses.
            }
          }
          tracing::warn!("[ serve_iface ] Error serving {:?} addr {:?} port {}: {:?}", iface_name, multicast_addr, port, e);
        }
      });
    }
  }

  tasks.join_all().await;

  Ok(())
}

#[allow(unreachable_code)]
pub async fn serve_iface(iface_idx: u32, iface_name: &str, iface_addrs: &Vec<std::net::IpAddr>, multicast_addr: &std::net::IpAddr, port: u16) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

  if crate::v_is_everything() {
    tracing::warn!("Attemting to bind to {} - {: <18} address {} port {} (addresses - {:?})", iface_idx, iface_name, multicast_addr, port, iface_addrs);
  }

  let empty_bind_addr_port = if multicast_addr.is_ipv4() {
    (std::net::IpAddr::V4(core::net::Ipv4Addr::new(0, 0, 0, 0)), port )
  }
  else {
    (std::net::IpAddr::V6(core::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)), port )
  };

  //let sock = tokio::net::UdpSocket::bind(empty_bind_addr_port).await.map_err(map_loc_err!())?;

  if let Ok(sock) = tokio::net::UdpSocket::bind(empty_bind_addr_port).await {

    if crate::v_is_info() {
      tracing::warn!("Successfully bound to {} - {: <18} address {} port {} (addresses - {:?})", iface_idx, iface_name, multicast_addr, port, iface_addrs);
    }

    if multicast_addr.is_ipv4() {
      sock.set_multicast_loop_v4(true).map_err(map_loc_err!())?;
      sock.set_multicast_ttl_v4(4).map_err(map_loc_err!())?; // How many hops multicast can live for - default is just the immediate LAN we are attached to. TODO configure me from /etc/weveryware.toml l8ter
    }
    else {
      sock.set_multicast_loop_v6(true).map_err(map_loc_err!())?;
    }

    match multicast_addr {
      std::net::IpAddr::V4(multicast_addr) => {
        if iface_addrs.len() > 0 {
          for iface_addr in iface_addrs.iter() {
            if let std::net::IpAddr::V4(iface_addr_v4) = iface_addr {
              sock.join_multicast_v4(*multicast_addr, *iface_addr_v4).map_err(map_loc_err!())?;
            }
          }
        }
        else {
          sock.join_multicast_v4(*multicast_addr, core::net::Ipv4Addr::UNSPECIFIED).map_err(map_loc_err!())?;
        }
      }
      std::net::IpAddr::V6(multicast_addr) => {
        sock.join_multicast_v6(multicast_addr, iface_idx).map_err(map_loc_err!())?;
      }
    }

    let mut buf = [0; 16*1024];
    loop {
        let (len, addr) = sock.recv_from(&mut buf).await.map_err(map_loc_err!())?;
        //tracing::warn!("{:?} bytes received from {:?} => {:?}", len, addr, &buf[..len]);
        if crate::v_is_everything() {
          tracing::warn!("{:?} bytes received from {:?} => {:?}", len, addr, &buf[..len]);
        }
        else if crate::v_is_info() {
          tracing::warn!("{:?} bytes received from {:?}", len, addr);
        }

        match serde_bare::from_slice::<crate::messages::ExecuteRequest>(&buf[..len]) {
          Ok(execute_req) => {
            tracing::warn!("Got execute req: {:?}", execute_req);
          }
          Err(e) => {
            tracing::warn!("{:?}", e);
          }
        }

        //sock.connect(addr).await.map_err(map_loc_err!())?;  // forces routing decision on BSD and MacOS machines, which otherwise error during send_to with "Os { code: 49, kind: AddrNotAvailable, message: "Can't assign requested address" }"

        // let len = sock.send_to(&buf[..len], addr).await.map_err(map_loc_err!())?;
        // tracing::warn!("{:?} bytes sent", len);

    }
  }

  Ok(())
}




