use crate::config::{Config, PeerMetadata};

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
