//! A cross-platform interactive terminal driver, exposed to WASI programs through the `host::tty_*`
//! imports. It is the host-side half of "a program can be a real UI": crossterm gives us raw mode,
//! structured key events, and styled/positioned output on Windows (incl. legacy consoles via Virtual
//! Terminal), macOS, and Linux with no C dependencies.
//!
//! A launcher (e.g. the `chat` command) calls [`attach`] once, holds the returned [`TtyGuard`] for
//! the session (its Drop restores the terminal even if the guest traps), and hands the [`TtyHandle`]
//! to the executor. The executor's `host::tty_*` callbacks then drive the terminal on the guest's
//! behalf. Output is structured (move / clear / style / print) rather than raw ANSI, so it renders
//! identically across shells; input arrives as decoded events, not raw termios bytes.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::{cursor, event, style, terminal, QueueableCommand};

/// A single decoded terminal input event. Deliberately small - just what an interactive line/'`ui`'
/// program needs. Encoded for the guest by [`encode_event`].
#[derive(Debug, Clone)]
pub enum TtyEvent {
  Char(char),
  Enter,
  Backspace,
  Left,
  Right,
  Up,
  Down,
  CtrlC,
  CtrlD,
  Esc,
  Resize(u16, u16),
}

/// Event kind tags in the wire encoding handed to the guest via `host::tty_next_event`. Mirror any
/// change in the guest decoder (example-programs/chat.c).
pub mod event_kind {
  pub const CHAR: u8 = 1; // followed by the character's UTF-8 bytes
  pub const ENTER: u8 = 2;
  pub const BACKSPACE: u8 = 3;
  pub const LEFT: u8 = 4;
  pub const RIGHT: u8 = 5;
  pub const UP: u8 = 6;
  pub const DOWN: u8 = 7;
  pub const CTRL_C: u8 = 8;
  pub const CTRL_D: u8 = 9;
  pub const ESC: u8 = 10;
  pub const RESIZE: u8 = 11; // followed by cols(u16 LE), rows(u16 LE)
}

/// Encode an event as `[kind][payload...]` for the guest. Char carries UTF-8; Resize carries
/// cols,rows as little-endian u16s; everything else is a single tag byte.
pub fn encode_event(ev: &TtyEvent) -> Vec<u8> {
  let mut out = Vec::with_capacity(6);
  match ev {
    TtyEvent::Char(c) => {
      out.push(event_kind::CHAR);
      let mut b = [0u8; 4];
      out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
    }
    TtyEvent::Enter => out.push(event_kind::ENTER),
    TtyEvent::Backspace => out.push(event_kind::BACKSPACE),
    TtyEvent::Left => out.push(event_kind::LEFT),
    TtyEvent::Right => out.push(event_kind::RIGHT),
    TtyEvent::Up => out.push(event_kind::UP),
    TtyEvent::Down => out.push(event_kind::DOWN),
    TtyEvent::CtrlC => out.push(event_kind::CTRL_C),
    TtyEvent::CtrlD => out.push(event_kind::CTRL_D),
    TtyEvent::Esc => out.push(event_kind::ESC),
    TtyEvent::Resize(c, r) => {
      out.push(event_kind::RESIZE);
      out.extend_from_slice(&c.to_le_bytes());
      out.extend_from_slice(&r.to_le_bytes());
    }
  }
  out
}

/// Handle to the attached terminal, shared into the executor. Input events arrive on a channel fed by
/// a dedicated blocking reader thread; output goes through a buffered, mutex-guarded stdout.
pub struct TtyHandle {
  rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<TtyEvent>>,
  out: std::sync::Mutex<std::io::BufWriter<std::io::Stdout>>,
}

impl TtyHandle {
  /// Await the next input event, up to `timeout`. `None` means the timeout elapsed (or the input
  /// thread ended) - the guest should treat that as "no input this tick" and loop.
  pub async fn next_event(&self, timeout: std::time::Duration) -> Option<TtyEvent> {
    let mut rx = self.rx.lock().await;
    match tokio::time::timeout(timeout, rx.recv()).await {
      Ok(ev) => ev,
      Err(_) => None,
    }
  }

  /// Queue text at the cursor (call [`flush`](Self::flush) to present it).
  pub fn print(&self, s: &str) {
    if let Ok(mut o) = self.out.lock() {
      let _ = o.queue(style::Print(s));
    }
  }
  /// Move the cursor to a 0-based (col, row).
  pub fn move_to(&self, col: u16, row: u16) {
    if let Ok(mut o) = self.out.lock() {
      let _ = o.queue(cursor::MoveTo(col, row));
    }
  }
  /// Clear the whole screen.
  pub fn clear(&self) {
    if let Ok(mut o) = self.out.lock() {
      let _ = o.queue(terminal::Clear(terminal::ClearType::All));
    }
  }
  /// Set foreground/background colour (ANSI 0-15, or -1 to reset) and attributes (bit 0 = bold,
  /// bit 1 = underline, bit 2 = reverse; 0 resets attributes).
  pub fn style(&self, fg: i32, bg: i32, attrs: i32) {
    if let Ok(mut o) = self.out.lock() {
      let _ = o.queue(style::SetForegroundColor(ansi_color(fg)));
      let _ = o.queue(style::SetBackgroundColor(ansi_color(bg)));
      let attr = if attrs == 0 {
        style::Attribute::Reset
      } else if attrs & 0b001 != 0 {
        style::Attribute::Bold
      } else if attrs & 0b010 != 0 {
        style::Attribute::Underlined
      } else {
        style::Attribute::Reverse
      };
      let _ = o.queue(style::SetAttribute(attr));
    }
  }
  /// Present everything queued since the last flush.
  pub fn flush(&self) {
    if let Ok(mut o) = self.out.lock() {
      let _ = o.flush();
    }
  }
  /// Current (cols, rows), or (80, 24) if the size can't be determined.
  pub fn size(&self) -> (u16, u16) {
    terminal::size().unwrap_or((80, 24))
  }
}

/// Map an ANSI colour index (0-15) to a crossterm colour; anything else resets to the default.
fn ansi_color(c: i32) -> style::Color {
  match c {
    0 => style::Color::Black,
    1 => style::Color::DarkRed,
    2 => style::Color::DarkGreen,
    3 => style::Color::DarkYellow,
    4 => style::Color::DarkBlue,
    5 => style::Color::DarkMagenta,
    6 => style::Color::DarkCyan,
    7 => style::Color::Grey,
    8 => style::Color::DarkGrey,
    9 => style::Color::Red,
    10 => style::Color::Green,
    11 => style::Color::Yellow,
    12 => style::Color::Blue,
    13 => style::Color::Magenta,
    14 => style::Color::Cyan,
    15 => style::Color::White,
    _ => style::Color::Reset,
  }
}

/// Restores the terminal (leave alternate screen, show cursor, disable raw mode) and stops the input
/// thread when dropped - even on a panic or a guest trap, so the user's shell is never left wedged.
pub struct TtyGuard {
  stop: Arc<AtomicBool>,
  reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for TtyGuard {
  fn drop(&mut self) {
    self.stop.store(true, Ordering::SeqCst);
    if let Some(h) = self.reader.take() {
      let _ = h.join();
    }
    let mut out = std::io::stdout();
    let _ = out.queue(cursor::Show);
    let _ = out.queue(terminal::LeaveAlternateScreen);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
  }
}

/// Attach to the controlling terminal: enable raw mode, switch to the alternate screen, and spawn a
/// blocking thread that decodes crossterm events onto a channel. Returns the shared [`TtyHandle`] and
/// a [`TtyGuard`] the caller must keep alive for the session. Fails if there is no usable terminal
/// (e.g. stdin/stdout is a pipe), in which case the caller should fall back to non-interactive output.
pub fn attach() -> std::io::Result<(Arc<TtyHandle>, TtyGuard)> {
  terminal::enable_raw_mode()?;
  {
    let mut out = std::io::stdout();
    out.queue(terminal::EnterAlternateScreen)?;
    out.queue(cursor::Hide)?;
    out.flush()?;
  }

  let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
  let stop = Arc::new(AtomicBool::new(false));
  let reader = {
    let stop = stop.clone();
    std::thread::spawn(move || {
      // Poll so we can notice the stop flag instead of blocking forever in read().
      let tick = std::time::Duration::from_millis(50);
      while !stop.load(Ordering::SeqCst) {
        match event::poll(tick) {
          Ok(true) => match event::read() {
            Ok(ev) => {
              if let Some(mapped) = map_event(ev) {
                if tx.send(mapped).is_err() {
                  break; // receiver gone
                }
              }
            }
            Err(_) => break,
          },
          Ok(false) => {}   // no event this tick
          Err(_) => break,  // input error
        }
      }
    })
  };

  let handle = Arc::new(TtyHandle {
    rx: tokio::sync::Mutex::new(rx),
    out: std::sync::Mutex::new(std::io::BufWriter::new(std::io::stdout())),
  });
  Ok((handle, TtyGuard { stop, reader: Some(reader) }))
}

/// Translate a crossterm event into our small [`TtyEvent`] set, dropping events we don't model.
fn map_event(ev: event::Event) -> Option<TtyEvent> {
  use event::{Event, KeyCode, KeyEvent, KeyModifiers};
  match ev {
    Event::Resize(c, r) => Some(TtyEvent::Resize(c, r)),
    Event::Key(KeyEvent { code, modifiers, kind, .. }) => {
      // On Windows key events fire for both press and release; only act on presses.
      if kind != event::KeyEventKind::Press && kind != event::KeyEventKind::Repeat {
        return None;
      }
      let ctrl = modifiers.contains(KeyModifiers::CONTROL);
      match code {
        KeyCode::Char('c') if ctrl => Some(TtyEvent::CtrlC),
        KeyCode::Char('d') if ctrl => Some(TtyEvent::CtrlD),
        KeyCode::Char(c) => Some(TtyEvent::Char(c)),
        KeyCode::Enter => Some(TtyEvent::Enter),
        KeyCode::Backspace => Some(TtyEvent::Backspace),
        KeyCode::Left => Some(TtyEvent::Left),
        KeyCode::Right => Some(TtyEvent::Right),
        KeyCode::Up => Some(TtyEvent::Up),
        KeyCode::Down => Some(TtyEvent::Down),
        KeyCode::Esc => Some(TtyEvent::Esc),
        _ => None,
      }
    }
    _ => None,
  }
}
