use crate::tty::{encode_event, event_kind, TtyEvent};

#[test]
fn encodes_char_events_as_utf8() {
  assert_eq!(encode_event(&TtyEvent::Char('a')), vec![event_kind::CHAR, b'a']);
  // Multi-byte UTF-8 is preserved so the guest can rebuild the character.
  assert_eq!(encode_event(&TtyEvent::Char('é')), vec![event_kind::CHAR, 0xC3, 0xA9]);
}

#[test]
fn encodes_named_and_resize_events() {
  assert_eq!(encode_event(&TtyEvent::Enter), vec![event_kind::ENTER]);
  assert_eq!(encode_event(&TtyEvent::Backspace), vec![event_kind::BACKSPACE]);
  assert_eq!(encode_event(&TtyEvent::CtrlC), vec![event_kind::CTRL_C]);
  // Resize carries cols,rows as little-endian u16s.
  assert_eq!(encode_event(&TtyEvent::Resize(80, 24)), vec![event_kind::RESIZE, 80, 0, 24, 0]);
  assert_eq!(encode_event(&TtyEvent::Resize(258, 1)), vec![event_kind::RESIZE, 2, 1, 1, 0]);
}
