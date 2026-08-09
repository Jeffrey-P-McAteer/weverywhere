use crate::crypto_utils::{format_public_key, public_key_to_ed25519_vk};

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
