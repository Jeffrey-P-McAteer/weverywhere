
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::*;

pub enum Format {
  JSON,
  CBOR,
  BARE,
}


/// General-purpose parser for network messages into our structs.
/// Use this to parse the first message from an unseen client.
pub fn deserialize<T:DeserializeOwned>(network_message: &[u8]) -> DynResult<(T, Format)> {
  for de_func in &[deserialize_json::<T>, deserialize_cbor::<T>, deserialize_bare::<T>] {
    if let Ok((parsed_t, format)) = de_func(network_message) {
      return Ok((parsed_t, format));
    }
  }
  let mut all_errs = String::with_capacity(4096);
  for de_func in &[deserialize_json::<T>, deserialize_cbor::<T>, deserialize_bare::<T>] {
    if let Err(e) = de_func(network_message) {
      let msg = format!("{:?}\n", e);
      all_errs.push_str(&msg);
    }
  }
  Err(format!("Data could not be parsed; attempted formats JSON, CBOR, BARE.\nParse errors:\n{:?}", all_errs).into())
}

/// This directly calls the expected format deserializer, avoiding work when we can expect
/// the message format to be a previously-observed value.
/// Use this to parse subsequent messages from a client after we know an assumed format.
pub fn deserialize_expected<T:DeserializeOwned>(network_message: &[u8], expected_format: &Format) -> DynResult<(T, Format)> {
  match expected_format {
    Format::JSON => {
      if let Ok((parsed_t, format)) = deserialize_json(network_message) {
        return Ok((parsed_t, format));
      }
    }
    Format::CBOR => {
      if let Ok((parsed_t, format)) = deserialize_cbor(network_message) {
        return Ok((parsed_t, format));
      }
    }
    Format::BARE => {
      if let Ok((parsed_t, format)) = deserialize_bare(network_message) {
        return Ok((parsed_t, format));
      }
    }
  }
  return deserialize(network_message);
}


fn deserialize_json<T:DeserializeOwned>(network_message: &[u8]) -> DynResult<(T, Format)> {
  let s = str::from_utf8(network_message)?;
  let t = serde_json::from_str(s)?;
  Ok((t, Format::JSON))
}

fn deserialize_cbor<T:DeserializeOwned>(network_message: &[u8]) -> DynResult<(T, Format)> {
  let mut deserializer = serde_cbor::Deserializer::from_slice(network_message);
  let t: T = serde::de::Deserialize::deserialize(&mut deserializer)?;
  Ok((t, Format::CBOR))
}

fn deserialize_bare<T:DeserializeOwned>(network_message: &[u8]) -> DynResult<(T, Format)> {
  let t = serde_bare::from_slice(network_message)?;
  Ok((t, Format::BARE))
}




pub fn serialize<T:Serialize>(t: &T, format_type: &Format) -> DynResult<Vec<u8>> {
  match format_type {
    Format::JSON => {
      let s = serde_json::to_string(t)?;
      return Ok(s.into_bytes());
    }
    Format::CBOR => {
      return Ok(serde_cbor::to_vec(t)?);
    }
    Format::BARE => {
      return Ok(serde_bare::to_vec(t)?);
    }
  }
}

