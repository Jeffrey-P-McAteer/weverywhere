
use super::*;

use core::pin::Pin;
use core::task::Poll;
use core::task::Context;

/// This will eventually be replaced with a better PKI-focused solution,
/// but for now we're simply streaming bytes back over UDP to the client
#[derive(Clone)]
pub struct WasiStdioSimpleForwarder {
  our_pid: u64,
  reply_to: Option<std::net::SocketAddr>,
  reply_from: Option<command::serve::UdpSocketSender>,
}


impl WasiStdioSimpleForwarder {
  pub fn new_nop() -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      our_pid: 0,
      reply_to: None,
      reply_from: None,
    }
  }
  pub fn new_maybe_udp(reply_to: Option<std::net::SocketAddr>, reply_from: Option<command::serve::UdpSocketSender>) -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      our_pid: 0,
      reply_to: reply_to,
      reply_from: reply_from,
    }
  }
  pub fn new_udp(reply_to: std::net::SocketAddr, reply_from: command::serve::UdpSocketSender) -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      our_pid: 0,
      reply_to: Some(reply_to),
      reply_from: Some(reply_from),
    }
  }
  pub fn set_pid(&mut self, pid: u64) {
    self.our_pid = pid;
  }
}


impl tokio::io::AsyncWrite for WasiStdioSimpleForwarder {
  fn poll_write(
      self: Pin<&mut Self>,
      cx: &mut Context<'_>,
      buf: &[u8],
  ) -> Poll<Result<usize, std::io::Error>> {
    if let (Some(reply_to), Some(reply_from)) = (self.reply_to, self.reply_from.clone()) {
      // Construct a
      let msg = messages::NetworkMessage::BasicInsecureProgramStdout {
        from_pid: self.our_pid,
        stdout_data: buf.to_vec(),
      };
      match serde_bare::to_vec(&msg) {
        Ok(msg_encoded) => {
          reply_from.poll_send_to(cx, &msg_encoded, reply_to)
        }
        Err(e) => {
          tracing::info!("e = {:?}", e);
          // Lie and say we wrote everything - this time b/c of an encoding error
          Poll::Ready(Ok(buf.len()))
        }
      }
    }
    else {
      // Lie and say we wrote everything - None,None becomes a no-op.
      Poll::Ready(Ok(buf.len()))
    }
  }
  fn poll_flush(
      self: Pin<&mut Self>,
      cx: &mut Context<'_>,
  ) -> Poll<Result<(), std::io::Error>> {
      // Lie and say we flushed everything - the network doesn't generally expose this
      Poll::Ready(Ok( () ))
  }
  fn poll_shutdown(
      self: Pin<&mut Self>,
      cx: &mut Context<'_>,
  ) -> Poll<Result<(), std::io::Error>> {
      // Again, we don't shut down the socket and an IP+Port pair isn't a stream, so this is also a no-op.
      Poll::Ready(Ok( () ))
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
