//! Shared rules for recursive, multi-hop discovery/query programs (e.g. `netmap`).
//!
//! A discovery program is sent to a node, which executes it and may forward it onward to its own
//! peers, building a tree. Two safety limits keep that from looping or amplifying without bound:
//!
//!   * a per-hop **depth budget**, capped by the trust relationship at each hop (this module), and
//!   * a **visited-set keyed by each server's identity public key**, so a node already present on
//!     the path is never re-queried or re-printed (enforced where the relay/aggregation lives).
//!
//! Depth policy: a node reached over a *trusted* edge may forward up to [`TRUSTED_FORWARD_DEPTH`]
//! hops; over an *untrusted* edge only [`UNTRUSTED_FORWARD_DEPTH`]. Trust is evaluated per edge, so
//! the budget collapses the moment a trusted chain crosses into an untrusted host: e.g. a trusted
//! host (budget 8) forwarding to an untrusted peer grants it only 2, an effective 3-hop subtree.

use crate::DynResult;

/// Hops a node reached over a trusted edge may still forward.
pub const TRUSTED_FORWARD_DEPTH: u8 = 8;

/// Hops a node reached over an untrusted edge may still forward.
pub const UNTRUSTED_FORWARD_DEPTH: u8 = 2;

/// The depth budget the origin (the machine running `netmap`/`run`) grants a first-hop peer, based
/// on whether the origin trusts that peer's key.
pub fn initial_depth_budget(origin_trusts_peer: bool) -> u8 {
  trust_cap(origin_trusts_peer)
}

/// A cryptographically-random 16-byte request UUID. The origin generates one per discovery; each
/// forwarding node generates its own for onward sub-requests (and rewrites replies back to its
/// caller's). See network-map.c, which seeds its own via `host::random`.
pub fn random_uuid16() -> [u8; 16] {
  use rand::RngCore;
  let mut u = [0u8; 16];
  rand::rngs::OsRng.fill_bytes(&mut u);
  u
}

/// The depth budget a forwarding node grants a peer it forwards to. `parent_remaining` is the budget
/// the forwarding node itself was granted; the grant is one less than that, but never more than the
/// trust cap for this edge. Returns 0 when no further forwarding is allowed (the peer still executes
/// and reports itself; it just won't recurse).
pub fn child_depth_budget(parent_remaining: u8, forwarder_trusts_peer: bool) -> u8 {
  if parent_remaining == 0 {
    return 0;
  }
  (parent_remaining - 1).min(trust_cap(forwarder_trusts_peer))
}

fn trust_cap(trusted: bool) -> u8 {
  if trusted { TRUSTED_FORWARD_DEPTH } else { UNTRUSTED_FORWARD_DEPTH }
}


// ============================================================================================
// Node attestation: a per-node signed statement binding (hostname, identity pubkey, timestamp).
//
// Each node executing the discovery program returns one of these so the caller can prove every
// entry in the map was produced by the node it claims to be - no host can represent more than
// itself, because the signature only verifies against that node's own identity key. The client
// also checks the timestamp falls inside the request window (see attestation_time_ok).
//
// The attestation is a CBOR map with small integer keys (compact + easy to hand-encode in the C
// discovery program). The daemon builds+signs it (host::signed_attestation); the client decodes
// and verifies it. Both sides share the canonical signing bytes below so signatures line up.
// ============================================================================================

/// CBOR integer keys inside an attestation map.
pub mod attest_keys {
  pub const HOSTNAME: i128 = 1;
  pub const PUBKEY: i128 = 2;
  pub const EPOCH_S: i128 = 3;
  pub const SIGNATURE: i128 = 4;
}

/// CBOR integer keys inside a per-node record map (what a node returns as its BasicReturnMap).
pub mod record_keys {
  /// Byte string: the CBOR-encoded attestation map (see [`attest_keys`]).
  pub const ATTESTATION: i128 = 1;
  /// uint 0/1: whether the node trusts the caller that sent it the program.
  pub const TRUSTS_CALLER: i128 = 2;
  /// uint: hop distance from the origin (origin's direct responders are depth 1).
  pub const DEPTH: i128 = 3;
  /// byte string: the identity pubkey of this node's caller (its parent in the tree).
  pub const PARENT_PUBKEY: i128 = 4;
}

/// Half-window (seconds) a node's attestation timestamp may deviate from the verifier's clock: the
/// spec's "60 second request window (plus or minus 30s)".
pub const ATTESTATION_WINDOW_HALF_S: u64 = 30;

/// Canonical bytes signed by a node's identity key to attest (hostname, pubkey, epoch). Both the
/// signer (daemon) and verifier (client) build these identically so ed25519 verification matches.
pub fn attestation_signing_bytes(hostname: &str, pubkey: &[u8], epoch_s: u64) -> Vec<u8> {
  let mut m = Vec::with_capacity(32 + hostname.len() + pubkey.len() + 8);
  m.extend_from_slice(b"weverywhere-node-attest-v1\n");
  m.extend_from_slice(hostname.as_bytes());
  m.push(b'\n');
  m.extend_from_slice(pubkey);
  m.push(b'\n');
  m.extend_from_slice(&epoch_s.to_le_bytes());
  m
}

/// Build the CBOR attestation map bytes from its already-computed parts (daemon side).
pub fn build_attestation_cbor(hostname: &str, pubkey: &[u8], epoch_s: u64, signature: &[u8]) -> DynResult<Vec<u8>> {
  use serde_cbor::Value;
  let map = Value::Map(
    [
      (Value::Integer(attest_keys::HOSTNAME), Value::Text(hostname.to_string())),
      (Value::Integer(attest_keys::PUBKEY), Value::Bytes(pubkey.to_vec())),
      (Value::Integer(attest_keys::EPOCH_S), Value::Integer(epoch_s as i128)),
      (Value::Integer(attest_keys::SIGNATURE), Value::Bytes(signature.to_vec())),
    ]
    .into_iter()
    .collect(),
  );
  Ok(serde_cbor::to_vec(&map)?)
}

/// A node attestation whose signature has been verified against its own identity key.
#[derive(Debug, Clone)]
pub struct VerifiedNode {
  pub hostname: String,
  pub pubkey: Vec<u8>,
  pub epoch_s: u64,
}

/// True if `epoch_s` is within +/- [`ATTESTATION_WINDOW_HALF_S`] of `now_epoch_s`.
pub fn attestation_time_ok(epoch_s: u64, now_epoch_s: u64) -> bool {
  let diff = if epoch_s > now_epoch_s { epoch_s - now_epoch_s } else { now_epoch_s - epoch_s };
  diff <= ATTESTATION_WINDOW_HALF_S
}

/// Decode + verify a CBOR attestation (client side): checks the ed25519 signature over the canonical
/// bytes. On success returns the (hostname, pubkey, epoch). Does NOT check the time window - the
/// caller does that separately so it can warn without discarding the node. Returns Err with a
/// human-readable reason on a malformed or badly-signed attestation.
pub fn verify_attestation_cbor(bytes: &[u8]) -> Result<VerifiedNode, String> {
  use serde_cbor::Value;
  let val: Value = serde_cbor::from_slice(bytes).map_err(|e| format!("cbor decode: {e}"))?;
  let map = match val { Value::Map(m) => m, _ => return Err("attestation is not a CBOR map".into()) };
  let get = |k: i128| map.get(&Value::Integer(k));

  let hostname = match get(attest_keys::HOSTNAME) {
    Some(Value::Text(s)) => s.clone(),
    _ => return Err("attestation missing hostname".into()),
  };
  let pubkey = match get(attest_keys::PUBKEY) {
    Some(Value::Bytes(b)) => b.clone(),
    _ => return Err("attestation missing pubkey".into()),
  };
  let epoch_s = match get(attest_keys::EPOCH_S) {
    Some(Value::Integer(i)) if *i >= 0 => *i as u64,
    _ => return Err("attestation missing/invalid epoch".into()),
  };
  let signature = match get(attest_keys::SIGNATURE) {
    Some(Value::Bytes(b)) => b.clone(),
    _ => return Err("attestation missing signature".into()),
  };

  let key_bytes: [u8; 32] = pubkey.as_slice().try_into().map_err(|_| "pubkey is not 32 bytes".to_string())?;
  let vk = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).map_err(|e| format!("bad pubkey: {e}"))?;
  let sig = ed25519_dalek::Signature::from_slice(&signature).map_err(|e| format!("bad signature: {e}"))?;
  let msg = attestation_signing_bytes(&hostname, &pubkey, epoch_s);
  use ed25519_dalek::Verifier;
  vk.verify(&msg, &sig).map_err(|e| format!("signature does not verify: {e}"))?;

  Ok(VerifiedNode { hostname, pubkey, epoch_s })
}
