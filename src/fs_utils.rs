
use crate::*;

pub fn format_size_bytes(size: usize) -> String {
  if size < 1_000 {
    format!("{}b", size)
  }
  else if size < 1_000_000 {
    format!("{:.1}kb", size as f32 / 1_000.0f32)
  }
  else if size < 1_000_000_000 {
    format!("{:.1}mb", size as f32 / 1_000_000.0f32)
  }
  else {
    format!("{:.1}gb", size as f32 / 1_000_000_000.0f32)
  }
}

