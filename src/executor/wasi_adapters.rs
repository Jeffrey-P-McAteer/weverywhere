
use super::*;

use core::pin::Pin;
use core::task::Poll;
use core::task::Context;
use std::io::Error;

/// This will eventually be replaced with a better PKI-focused solution,
/// but for now we're simply streaming bytes back over UDP to the client
#[derive(Debug,Clone)]
pub struct WasiStdioSimpleForwarder {
  reply_to: Option<std::net::SocketAddr>,
}


impl WasiStdioSimpleForwarder {
  pub fn new_maybe_udp(reply_to: Option<std::net::SocketAddr>) -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      reply_to: reply_to,
    }
  }
  pub fn new_udp(reply_to: std::net::SocketAddr) -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      reply_to: Some(reply_to),
    }
  }
}


impl tokio::io::AsyncWrite for WasiStdioSimpleForwarder {
  fn poll_write(
      self: Pin<&mut Self>,
      cx: &mut Context<'_>,
      buf: &[u8],
  ) -> Poll<Result<usize, Error>> {
    std::unimplemented!()
  }
  fn poll_flush(
      self: Pin<&mut Self>,
      cx: &mut Context<'_>,
  ) -> Poll<Result<(), Error>> {
    std::unimplemented!()
  }
  fn poll_shutdown(
      self: Pin<&mut Self>,
      cx: &mut Context<'_>,
  ) -> Poll<Result<(), Error>> {
    std::unimplemented!()
  }

}

impl wasmtime_wasi::cli::IsTerminal for WasiStdioSimpleForwarder {
  fn is_terminal(&self) -> bool {
    false
  }
}

impl wasmtime_wasi::cli::StdoutStream for WasiStdioSimpleForwarder {
  fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
    return Box::new(self.clone());
  }
}
