use crate::discovery::*;

#[test]
fn trusted_chain_rides_full_depth() {
  // Origin trusts A (budget 8); a fully-trusted chain decrements by one each hop.
  let mut d = initial_depth_budget(true);
  assert_eq!(d, 8);
  let mut hops = 0;
  while d > 0 {
    d = child_depth_budget(d, true);
    hops += 1;
  }
  assert_eq!(hops, 8, "a trusted chain should reach 8 forwarding hops");
}

#[test]
fn untrusted_first_hop_only_two() {
  let mut d = initial_depth_budget(false);
  assert_eq!(d, 2);
  let mut hops = 0;
  while d > 0 {
    d = child_depth_budget(d, false);
    hops += 1;
  }
  assert_eq!(hops, 2, "an untrusted chain should reach 2 forwarding hops");
}

#[test]
fn trusted_to_untrusted_collapses_to_two() {
  // Trusted host has budget 8; forwarding to an untrusted peer collapses the grant to 2,
  // an effective 3-hop subtree below the trusted host (the hop to the untrusted peer + 2 more).
  let trusted = initial_depth_budget(true); // 8
  let untrusted_child = child_depth_budget(trusted, false);
  assert_eq!(untrusted_child, 2);
  // ...and that untrusted node's own untrusted children keep collapsing toward zero.
  assert_eq!(child_depth_budget(untrusted_child, false), 1);
  assert_eq!(child_depth_budget(1, false), 0);
}

#[test]
fn remaining_beats_trust_cap_when_smaller() {
  // A trusted edge can't grant more than the parent had left.
  assert_eq!(child_depth_budget(2, true), 1);
  assert_eq!(child_depth_budget(0, true), 0);
}

#[test]
fn attestation_signs_and_verifies_and_binds_identity() {
  use ed25519_dalek::Signer;
  use rand::rngs::OsRng;

  let signing = ed25519_dalek::SigningKey::generate(&mut OsRng);
  let pubkey = signing.verifying_key().as_bytes().to_vec();
  let epoch = 1_760_000_000u64;

  // Daemon side: sign the canonical bytes and pack the CBOR attestation.
  let sig = signing.sign(&attestation_signing_bytes("node1", &pubkey, epoch)).to_bytes().to_vec();
  let cbor = build_attestation_cbor("node1", &pubkey, epoch, &sig).expect("build");

  // Client side: verify.
  let node = verify_attestation_cbor(&cbor).expect("verify");
  assert_eq!(node.hostname, "node1");
  assert_eq!(node.pubkey, pubkey);
  assert_eq!(node.epoch_s, epoch);

  // Timestamp window: within +/-30s ok, outside not.
  assert!(attestation_time_ok(epoch, epoch + 30));
  assert!(attestation_time_ok(epoch, epoch - 30));
  assert!(!attestation_time_ok(epoch, epoch + 31));

  // Tampering the hostname must break verification (a host can't represent another).
  let mut tampered: serde_cbor::Value = serde_cbor::from_slice(&cbor).unwrap();
  if let serde_cbor::Value::Map(m) = &mut tampered {
    m.insert(serde_cbor::Value::Integer(attest_keys::HOSTNAME), serde_cbor::Value::Text("evil".into()));
  }
  let tampered_bytes = serde_cbor::to_vec(&tampered).unwrap();
  assert!(verify_attestation_cbor(&tampered_bytes).is_err());
}
