use crate::config::{Config, PeerMetadata, default_identity_keyfile};

#[test]
fn identity_keyfile_is_optional_and_defaults() {
  // An ahead-of-time config (like a VM's initial-weverywhere.toml) may omit `keyfile`; it must
  // still parse and fall back to the platform default so generate-missing-keys can populate it.
  let cfg: Config = toml::from_str(
    r#"
      [identity]
      name = "win-test01-from-config-file"

      [[peer]]
      ipv4 = "10.0.0.2"
    "#,
  )
  .expect("config without keyfile should parse");
  assert_eq!(cfg.identity.name, "win-test01-from-config-file");
  assert_eq!(cfg.identity.keyfile, default_identity_keyfile());
  assert_eq!(cfg.peer.len(), 1);
}

#[test]
fn parses_all_forms_and_prefers_hostname_then_v6_then_v4() {
  let cfg: Config = toml::from_str(
    r#"
      [identity]
      name = "t"
      keyfile = "/tmp/x.pem"

      [[peer]]
      hostname = "node1.example"
      ipv6 = "fe80::1"
      ipv4 = "10.0.0.1"
      expected_key = { key = "ssh-ed25519 AAAAKEY" }
    "#,
  )
  .expect("valid config");

  assert_eq!(cfg.peer.len(), 1);
  let p = &cfg.peer[0];
  assert_eq!(p.hostname.as_deref(), Some("node1.example"));
  assert_eq!(p.ipv6, Some("fe80::1".parse().unwrap()));
  assert_eq!(p.ipv4, Some("10.0.0.1".parse().unwrap()));
  assert_eq!(p.expected_key_str(), Some("ssh-ed25519 AAAAKEY"));
  // Preference order: hostname, then ipv6, then ipv4.
  assert_eq!(p.connect_hosts(), vec!["node1.example", "fe80::1", "10.0.0.1"]);
}

#[test]
fn ipv6_preferred_over_ipv4_when_no_hostname() {
  let p: PeerMetadata = toml::from_str(
    r#"ipv4 = "192.0.2.7"
       ipv6 = "2001:db8::5""#,
  )
  .unwrap();
  assert_eq!(p.connect_hosts(), vec!["2001:db8::5", "192.0.2.7"]);
  assert!(p.expected_key.is_none());
}

#[test]
fn requires_at_least_one_address_form() {
  // No hostname/ipv4/ipv6 -> rejected.
  let err = toml::from_str::<PeerMetadata>(r#"expected_key = { key = "k" }"#).unwrap_err();
  assert!(err.to_string().contains("at least one"), "got: {err}");
  // A blank hostname does not count as "set".
  let err = toml::from_str::<PeerMetadata>(r#"hostname = "   ""#).unwrap_err();
  assert!(err.to_string().contains("at least one"), "got: {err}");
}

#[test]
fn rejects_malformed_ip_addresses() {
  assert!(toml::from_str::<PeerMetadata>(r#"ipv4 = "not-an-ip""#).is_err());
  assert!(toml::from_str::<PeerMetadata>(r#"ipv6 = "10.0.0.1""#).is_err());
}

#[test]
fn pinned_toml_round_trips_back_into_config() {
  let p: PeerMetadata = toml::from_str(
    r#"hostname = "node1"
       ipv4 = "10.0.0.9""#,
  )
  .unwrap();
  let block = p.to_pinned_toml("ssh-ed25519 AAAAOBSERVED");
  // The generated block names the peer by its set fields and pins the key...
  assert!(block.starts_with("[[peer]]\n"));
  assert!(block.contains("hostname = \"node1\""));
  assert!(block.contains("ipv4 = \"10.0.0.9\""));
  assert!(block.contains("expected_key = { key = \"ssh-ed25519 AAAAOBSERVED\" }"));
  assert!(!block.contains("ipv6"));
  // ...and it parses straight back into a Config as a valid [[peer]] entry.
  let cfg: Config = toml::from_str(&format!(
    "[identity]\nname=\"t\"\nkeyfile=\"/tmp/x.pem\"\n\n{block}"
  ))
  .expect("generated [[peer]] block should be valid TOML");
  assert_eq!(cfg.peer[0].expected_key_str(), Some("ssh-ed25519 AAAAOBSERVED"));
}

#[test]
fn signed_message_payload_roundtrips_and_detects_tampering() {
  // Backs host::messages_send / SignedFabricMessage: a payload signed by an identity's key must
  // verify against that identity, and any change to the payload, nonce, or signer must fail.
  use crate::config::IdentityData;
  use ed25519_dalek::SigningKey;
  use rand::rngs::OsRng;

  let signing = SigningKey::generate(&mut OsRng);
  let pubkey = signing.verifying_key().as_bytes().to_vec();
  // A minimal identity whose encoded_public_key is the verifying key (the fields verify_payload uses).
  let identity = IdentityData {
    human_name: "alice".into(),
    generated_at_utc0_epoch_s: 1_760_000_000,
    validity_s: u16::MAX,
    encoded_public_key_fmt: "ed25519".into(),
    encoded_public_key: pubkey.clone(),
    signature: vec![],
  };

  let id = [7u8; 16];
  let payload = b"\x81\x64test"; // CBOR: array(1) [ "test" ]
  let sig = IdentityData::sign_payload(&signing, &id, payload).to_bytes().to_vec();

  // Genuine message verifies.
  assert!(identity.verify_payload(&id, payload, &sig).is_ok());
  // Tampered body fails.
  assert!(identity.verify_payload(&id, b"\x81\x64evil", &sig).is_err());
  // Different nonce (replay under a new id) fails.
  assert!(identity.verify_payload(&[9u8; 16], payload, &sig).is_err());
  // A different signer's key can't be forged onto our identity.
  let other = SigningKey::generate(&mut OsRng);
  let other_sig = IdentityData::sign_payload(&other, &id, payload).to_bytes().to_vec();
  assert!(identity.verify_payload(&id, payload, &other_sig).is_err());
}
