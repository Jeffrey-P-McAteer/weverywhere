use crate::crypto_utils::{format_public_key, public_key_to_ed25519_vk, short_id, to_hex};

// Round-trip a freshly generated ed25519 key through the OpenSSH string form we use for
// [[trusted]] / [[peer]] `expected_key` values, proving format_public_key and
// public_key_to_ed25519_vk agree (they must, for peer-key pinning to work).
#[test]
fn public_key_openssh_string_round_trips() {
  use rand::rngs::OsRng;
  let signing = ed25519_dalek::SigningKey::generate(&mut OsRng);
  let openssh = format_public_key(&signing);
  assert!(openssh.starts_with("ssh-ed25519 "), "got: {openssh}");
  let parsed = public_key_to_ed25519_vk(&openssh).expect("parse back");
  assert_eq!(parsed.as_bytes(), signing.verifying_key().as_bytes());
}

// The short-id shown in netmap and the daemon's security logs MUST be the first 4 pubkey bytes as 8
// lowercase hex chars - exactly what chat.c renders - so the same identity is recognizable across
// applications. This pins that contract; changing it would silently desync chat from netmap.
#[test]
fn short_id_is_first_four_bytes_hex_matching_chat() {
  let pubkey = [0x39, 0xad, 0x41, 0x76, 0xde, 0xad, 0xbe, 0xef];
  assert_eq!(short_id(&pubkey), "39ad4176");
  assert_eq!(to_hex(&pubkey), "39ad4176deadbeef");
  // Robust for undersized keys (never panics): a short key hexes what it has.
  assert_eq!(short_id(&[0x39, 0xad]), "39ad");
  assert_eq!(short_id(&[]), "");
}
