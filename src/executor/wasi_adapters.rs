
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

  // Polled state
  current_encoded_msg: Option<Vec<u8>>,
  current_encoded_sent: usize,

}

impl std::fmt::Debug for WasiStdioSimpleForwarder {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      f.debug_struct("WasiStdioSimpleForwarder")
          .field("our_pid", &self.our_pid)
          .field("reply_to", &self.reply_to)
          // reply_from does not impl Debug
          .finish()
  }
}


impl WasiStdioSimpleForwarder {
  pub fn new_nop() -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      our_pid: 0,
      reply_to: None,
      reply_from: None,
      current_encoded_msg: None,
      current_encoded_sent: 0
    }
  }
  pub fn new_maybe_udp(reply_to: Option<std::net::SocketAddr>, reply_from: Option<command::serve::UdpSocketSender>) -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      our_pid: 0,
      reply_to: reply_to,
      reply_from: reply_from,
      current_encoded_msg: None,
      current_encoded_sent: 0
    }
  }
  pub fn new_udp(reply_to: std::net::SocketAddr, reply_from: command::serve::UdpSocketSender) -> WasiStdioSimpleForwarder {
    WasiStdioSimpleForwarder {
      our_pid: 0,
      reply_to: Some(reply_to),
      reply_from: Some(reply_from),
      current_encoded_msg: None,
      current_encoded_sent: 0
    }
  }
  pub fn set_pid(&mut self, pid: u64) {
    self.our_pid = pid;
  }
}


impl tokio::io::AsyncWrite for WasiStdioSimpleForwarder {
  fn poll_write(
      mut self: Pin<&mut Self>,
      cx: &mut Context<'_>,
      buf: &[u8],
  ) -> Poll<Result<usize, std::io::Error>> {
    if let (Some(reply_to), Some(reply_from)) = (self.reply_to, self.reply_from.clone()) {
      if self.current_encoded_msg.is_none() {
        let msg = messages::NetworkMessage::BasicInsecureProgramStdout {
          from_pid: self.our_pid,
          stdout_data: buf.to_vec(),
        };
        match serde_bare::to_vec(&msg) {
          Ok(msg_encoded) => {
            self.current_encoded_sent = 0;
            self.current_encoded_msg = Some(msg_encoded);
          }
          Err(e) => {
            tracing::info!("e = {:?}", e);
            // Lie and say we wrote everything - We will handle encoding errors l8ter
            return Poll::Ready(Ok(buf.len()));
          }
        }
        if let Some(frame) = self.current_encoded_msg.clone() { // Todo better memory management!
          while self.current_encoded_sent < frame.len() {
            let n = match reply_from.poll_send_to(
                cx,
                &frame[self.current_encoded_sent..],
                reply_to,
            ) {
                Poll::Ready(Ok(n)) => n,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            // UDP usually sends whole frames, but never assume
            self.current_encoded_sent += n;
          }
        }
      }

      // Either we errored when encoding, or Success, we have sent everything! Clear internal poll state and tell poller we are done.
      self.current_encoded_msg = None;
      Poll::Ready(Ok(buf.len()))
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
