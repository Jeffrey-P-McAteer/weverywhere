
use super::*;


pub async fn run(args: &args::Args, file_path: &std::path::PathBuf, multicast_groups: args::MulticastAddressVec, port: u16) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

  // Step 1: Read the executable material & form an exeute request object, sign it, and transmit.
  let wasm_bytes = tokio::fs::read(file_path).await.map_err(map_loc_err!())?;

  let local_config = config::Config::read_from_file(&args.config).await.map_err(map_loc_err!())?;

  let source = config::IdentityData::generate_from_config(&local_config).await.map_err(map_loc_err!())?;

  let pd = executor::ProgramDataBuilder::new()
    .set_human_name(
      file_path.file_name().map(|fn_osstr| fn_osstr.to_string_lossy().to_string() ).unwrap_or_else(|| "UNSET_NAME".to_string() )
    )
    .set_wasm_program_bytes(&wasm_bytes)
    .set_source(&source)
    .build().map_err(map_loc_err!())?;

  let execute_req = messages::NetworkMessage::ExecuteRequest {
    program_data: pd.clone(),
  };
  let execute_req_encoded = serde_bare::to_vec(&execute_req)?;

  // Step 2: Transmit to all multicast groups on all interfaces
  let mut tasks = tokio::task::JoinSet::new();
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

pub async fn run_one_iface(ex_req_bytes: &[u8], pd: &executor::ProgramData, iface_idx: u32, iface_name: &str, iface_addrs: &Vec<std::net::IpAddr>, multicast_group: &std::net::IpAddr, port: u16) -> DynResult<()> {

  if crate::v_is_info() {
    tracing::warn!("Sending {} bytes to {:?} port {} on iface {} ({:?})", 999, multicast_group, port, iface_name, iface_addrs);
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
  let mut buf = [0; 1024];

  let len = sock.send_to(&ex_req_bytes, (*multicast_group, port)).await.map_err(map_loc_err!())?;
  tracing::warn!("{:?} bytes sent", len);

  let td = tokio::time::Duration::from_millis(100);

  for _ in 0..24 {
    // Only wait up to 100ms for a reply;
    match tokio::time::timeout(td, sock.recv(&mut buf)).await {
      Ok(Ok(len)) => {
        tracing::warn!("{:?} bytes received from {:?} => {:?}", len, multicast_group, &buf[0..len]);
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
