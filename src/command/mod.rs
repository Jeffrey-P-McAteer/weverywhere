
use crate::*;
use crate::args::*;

pub async fn run_command(cmd: &args::Command, args: &args::Args) -> DynResult<()> {

  match cmd {
    Command::Info { file_path } => {
      info(file_path).await?;
    }
    Command::InstallTo { install_root, install_etc, install_bin } => {
      install_to(install_root, install_etc, install_bin).await?;
    }
    Command::Run { file_path, multicast_groups, port } => {
      run(file_path, multicast_groups.clone(), *port).await?;
    }
    Command::Serve { multicast_groups, port } => {
      serve(multicast_groups.clone(), *port).await?;
    }
  }

  Ok(())
}

pub async fn info(file_path: &std::path::PathBuf) -> DynResult<()> {
  eprintln!("This has not been implemented yet, see {}:{}", file!(), line!());

  Ok(())
}

pub async fn install_to(install_root: &std::path::PathBuf, install_etc: &Option<std::path::PathBuf>, install_bin: &Option<std::path::PathBuf>) -> DynResult<()> {
  eprintln!("This has not been implemented yet, see {}:{}", file!(), line!());
  Ok(())
}

pub async fn run(file_path: &std::path::PathBuf, multicast_groups: args::MulticastAddressVec, port: u16) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

  println!("Todo iterate ifaces and {:?} for port port", multicast_groups);

  /*
  println!("Sending to {}:{}", multicast_group, port);

  // TODO v6+v4 auto-detect?
  //let sock = tokio::net::UdpSocket::bind( "[::]:0" ).await?;
  let sock = tokio::net::UdpSocket::bind( "0.0.0.0:0" ).await?;

  sock.set_multicast_loop_v4(true)?;
  sock.set_multicast_ttl_v4(1)?; // How many hops multicast can live for - default is just the immediate LAN we are attached to. TODO configure me from /etc/weveryware.toml l8ter

  match multicast_group {
    std::net::IpAddr::V4(multicast_group) => {
      sock.join_multicast_v4(*multicast_group, core::net::Ipv4Addr::UNSPECIFIED)?;
    }
    std::net::IpAddr::V6(multicast_group) => {
      sock.join_multicast_v6(multicast_group, 0 /* unspecified */)?;
    }
  }

  // sock.connect( (*multicast_group, port) ).await?;
  let mut buf = [0; 1024];

  let len = sock.send_to(b"test 111111 test 222222 test 333333", (*multicast_group, port)).await?;
  println!("{:?} bytes sent", len);

  let td = tokio::time::Duration::from_millis(100);

  for _ in 0..24 {
    // Only wait up to 100ms for a reply;
    match tokio::time::timeout(td, sock.recv(&mut buf)).await {
      Ok(Ok(len)) => {
        println!("{:?} bytes received from {:?} => {:?}", len, multicast_group, &buf[0..len]);
      }
      Ok(Err(e)) => {
        // The socket operation itself failed
        eprintln!("Socket error: {e}");
      }
      Err(_) => {
        // The timeout expired (no data within 100ms)
        // println!("Timed out");
      }
    }
  }*/

  Ok(())
}

#[allow(unreachable_code)]
pub async fn serve(multicast_group: args::MulticastAddressVec, port: u16) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

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
          eprintln!("[ serve_iface ] Error serving {:?} addr {:?} port {}: {:?}", iface_name, multicast_addr, port, e);
        }
      });
    }
  }

  tasks.join_all().await;

  /*
  println!("Binding to {}:{}", multicast_group, port);

  let sock = tokio::net::UdpSocket::bind( "0.0.0.0:0" ).await?;

  sock.set_multicast_loop_v4(true)?;
  sock.set_multicast_ttl_v4(1)?; // How many hops multicast can live for - default is just the immediate LAN we are attached to. TODO configure me from /etc/weveryware.toml l8ter

  match multicast_group {
    std::net::IpAddr::V4(multicast_group) => {
      sock.join_multicast_v4(*multicast_group, core::net::Ipv4Addr::UNSPECIFIED)?;
    }
    std::net::IpAddr::V6(multicast_group) => {
      sock.join_multicast_v6(multicast_group, 0 /* unspecified */)?;
    }
  }

  let mut buf = [0; 16*1024];
  loop {
      let (len, addr) = sock.recv_from(&mut buf).await?;
      println!("{:?} bytes received from {:?} => {:?}", len, addr, &buf[..len]);

      //sock.connect(addr).await?;  // forces routing decision on BSD and MacOS machines, which otherwise error during send_to with "Os { code: 49, kind: AddrNotAvailable, message: "Can't assign requested address" }"

      let len = sock.send_to(&buf[..len], addr).await?;
      println!("{:?} bytes sent", len);

  }
  */

  Ok(())
}

#[allow(unreachable_code)]
pub async fn serve_iface(iface_idx: u32, iface_name: &str, iface_addrs: &Vec<std::net::IpAddr>, multicast_addr: &std::net::IpAddr, port: u16) -> DynResult<()> {
  use tokio::net::ToSocketAddrs;

  if crate::v_is_info() {
    println!("Binding to {} - {: <18} address {} port {} (addresses - {:?})", iface_idx, iface_name, multicast_addr, port, iface_addrs);
  }

  let empty_bind_addr_port = if multicast_addr.is_ipv4() {
    (std::net::IpAddr::V4(core::net::Ipv4Addr::new(0, 0, 0, 0)), 0 )
  }
  else {
    (std::net::IpAddr::V6(core::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)), 0 )
  };

  let sock = tokio::net::UdpSocket::bind(empty_bind_addr_port).await?;

  if multicast_addr.is_ipv4() {
    sock.set_multicast_loop_v4(true)?;
    sock.set_multicast_ttl_v4(1)?; // How many hops multicast can live for - default is just the immediate LAN we are attached to. TODO configure me from /etc/weveryware.toml l8ter
  }

  match multicast_addr {
    std::net::IpAddr::V4(multicast_addr) => {
      if iface_addrs.len() > 0 {
        for iface_addr in iface_addrs.iter() {
          if let std::net::IpAddr::V4(iface_addr_v4) = iface_addr {
            sock.join_multicast_v4(*multicast_addr, *iface_addr_v4)?;
          }
        }
      }
      else {
        sock.join_multicast_v4(*multicast_addr, core::net::Ipv4Addr::UNSPECIFIED)?;
      }
    }
    std::net::IpAddr::V6(multicast_addr) => {
      sock.join_multicast_v6(multicast_addr, iface_idx)?;
    }
  }

  let mut buf = [0; 16*1024];
  loop {
      let (len, addr) = sock.recv_from(&mut buf).await?;
      println!("{:?} bytes received from {:?} => {:?}", len, addr, &buf[..len]);

      //sock.connect(addr).await?;  // forces routing decision on BSD and MacOS machines, which otherwise error during send_to with "Os { code: 49, kind: AddrNotAvailable, message: "Can't assign requested address" }"

      let len = sock.send_to(&buf[..len], addr).await?;
      println!("{:?} bytes sent", len);

  }

  Ok(())
}




#[derive(serde::Deserialize, serde::Serialize)]
pub struct HelloWorld {
  pub message: String,
  pub misc: u32,
}



