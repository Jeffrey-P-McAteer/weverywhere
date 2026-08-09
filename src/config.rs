
use crate::*;

use optionable::OptionableConvert;


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct Config {
  pub identity: IdentityConfig,

  #[serde(default)]
  pub trusted: Vec<SingleTrustedKey>,

  #[serde(default)]
  pub startup_program: Vec<SingleStartupProgram>,

  #[serde(default)]
  pub includes: Vec<SingleInclude>,

  /// An initial, statically-configured network of peer nodes to send execution
  /// programs to, in addition to multicast discovery. Each `[[peer]]` entry is
  /// used alongside the multicast groups in every send/receive operation. See
  /// [`PeerMetadata`].
  #[serde(default)]
  pub peer: Vec<PeerMetadata>,

  #[serde(default)]
  pub limits: Limits,
}

#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct Limits {
  #[serde(default)]
  trusted: Limit,
  #[serde(default)]
  untrusted: Limit,
}

#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct Limit {
  #[serde(default)]
  max_cpu_instructions: u64,

  #[serde(default)]
  max_memory_bytes: u64,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct SingleInclude {
  pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct SingleTrustedKey {
  pub key: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct SingleStartupProgram {
  pub wasi_file: String,
}


/// One statically-configured peer node in `[[peer]]`. It is addressed by any
/// combination of a DNS `hostname`, an `ipv4` address, and an `ipv6` address -
/// at least one of the three MUST be set (enforced at deserialize time). The
/// `ipv4`/`ipv6` values are parsed as real addresses; `hostname` is any string
/// resolved by the socket layer at connect time.
///
/// When we actually connect we try the addresses in a fixed preference order -
/// `hostname`, then `ipv6`, then `ipv4` (see [`PeerMetadata::connect_hosts`]).
///
/// `expected_key` optionally pins the server's advertised public key. When it is
/// unset, the first time we reach the peer we log the key it actually advertised
/// as a ready-to-paste TOML block (see [`PeerMetadata::unpinned_key_warning`]) so
/// an operator can pin it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "PeerMetadataToml")]
pub struct PeerMetadata {
  /// DNS name; resolved by the socket layer at connect time. Preferred first.
  pub hostname: Option<String>,
  /// Parsed IPv6 address. Preferred over `ipv4` when both are present.
  pub ipv6: Option<std::net::Ipv6Addr>,
  /// Parsed IPv4 address. The last-resort connection target.
  pub ipv4: Option<std::net::Ipv4Addr>,
  /// Optionally-pinned expected public key for this peer (same OpenSSH
  /// `ssh-ed25519 <base64>` string form as `[[trusted]]` keys).
  pub expected_key: Option<SingleTrustedKey>,
}

/// Wire form of a `[[peer]]` entry. `PeerMetadata` is `#[serde(try_from)]` this
/// so we can (a) parse `ipv4`/`ipv6` as real addresses via their own Deserialize
/// impls and (b) enforce that at least one address form is present, normalising a
/// blank `hostname` to "unset".
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerMetadataToml {
  #[serde(default)]
  pub hostname: Option<String>,
  #[serde(default)]
  pub ipv6: Option<std::net::Ipv6Addr>,
  #[serde(default)]
  pub ipv4: Option<std::net::Ipv4Addr>,
  #[serde(default)]
  pub expected_key: Option<SingleTrustedKey>,
}

impl TryFrom<PeerMetadataToml> for PeerMetadata {
  type Error = String;
  fn try_from(raw: PeerMetadataToml) -> Result<Self, Self::Error> {
    // A present-but-blank hostname counts as unset (both for the "at least one"
    // rule and so we never try to connect to an empty host).
    let hostname = raw.hostname.and_then(|h| {
      let trimmed = h.trim();
      if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    });
    if hostname.is_none() && raw.ipv4.is_none() && raw.ipv6.is_none() {
      return Err(
        "each [[peer]] entry must set at least one of `hostname`, `ipv4`, or `ipv6`"
          .to_string(),
      );
    }
    Ok(PeerMetadata {
      hostname,
      ipv6: raw.ipv6,
      ipv4: raw.ipv4,
      expected_key: raw.expected_key,
    })
  }
}

// `Config` derives `optionable::Optionable`, whose generated `ConfigOpt` requires
// every field's element type to be `Optionable`. `PeerMetadata` contains
// `Ipv4Addr`/`Ipv6Addr`, which the crate does not implement `Optionable` for, so we
// cannot derive it. Instead we treat a peer as an atomic leaf (its `Optioned` is
// itself), exactly like the crate does for `String`/primitives. The config merge
// (`fancy_omerge_vec`) already concatenates `[[peer]]` across include files, so
// per-field optionality inside a peer would add nothing.
impl optionable::Optionable for PeerMetadata {
  type Optioned = Self;
}
impl optionable::OptionableConvert for PeerMetadata {
  fn into_optioned(self) -> Self::Optioned { self }
  fn try_from_optioned(value: Self::Optioned) -> Result<Self, optionable::Error> { Ok(value) }
  fn merge(&mut self, other: Self::Optioned) -> Result<(), optionable::Error> {
    *self = other;
    Ok(())
  }
}

impl PeerMetadata {
  /// Connection candidates in preference order: `hostname`, then `ipv6`, then
  /// `ipv4`. Each is a host string to pair with a port (DNS names are resolved by
  /// the socket layer). Always non-empty - the "at least one address" invariant is
  /// enforced at load time.
  pub fn connect_hosts(&self) -> Vec<String> {
    let mut hosts = Vec::with_capacity(3);
    if let Some(h) = self.hostname.as_deref() {
      hosts.push(h.to_string());
    }
    if let Some(v6) = &self.ipv6 {
      hosts.push(v6.to_string());
    }
    if let Some(v4) = &self.ipv4 {
      hosts.push(v4.to_string());
    }
    hosts
  }

  /// A short, stable identifier for this peer for log messages, listing whichever
  /// of hostname/ipv6/ipv4 are set (e.g. `hostname=node1 ipv6=fe80::1`).
  pub fn label(&self) -> String {
    let mut parts = Vec::with_capacity(3);
    if let Some(h) = self.hostname.as_deref() {
      parts.push(format!("hostname={h}"));
    }
    if let Some(v6) = &self.ipv6 {
      parts.push(format!("ipv6={v6}"));
    }
    if let Some(v4) = &self.ipv4 {
      parts.push(format!("ipv4={v4}"));
    }
    parts.join(" ")
  }

  /// The pinned key string, if any.
  pub fn expected_key_str(&self) -> Option<&str> {
    self.expected_key.as_ref().map(|k| k.key.as_str())
  }

  /// A ready-to-paste `[[peer]]` TOML block for this peer that pins
  /// `observed_key`. The peer is identified by whichever of hostname/ipv6/ipv4 it
  /// was configured with (same fields, same preference order).
  pub fn to_pinned_toml(&self, observed_key: &str) -> String {
    let mut s = String::from("[[peer]]\n");
    if let Some(h) = self.hostname.as_deref() {
      s.push_str(&format!("hostname = {}\n", toml_basic_string(h)));
    }
    if let Some(v6) = &self.ipv6 {
      s.push_str(&format!("ipv6 = {}\n", toml_basic_string(&v6.to_string())));
    }
    if let Some(v4) = &self.ipv4 {
      s.push_str(&format!("ipv4 = {}\n", toml_basic_string(&v4.to_string())));
    }
    s.push_str(&format!(
      "expected_key = {{ key = {} }}\n",
      toml_basic_string(observed_key)
    ));
    s
  }

  /// Warning to log the first time we reach a peer that has no `expected_key`
  /// pinned: it surfaces the key the server actually advertised and shows the
  /// exact `[[peer]]` TOML to paste to pin it. `observed_key` is the OpenSSH-form
  /// public key string the server advertised (see `crypto_utils::format_public_key`).
  pub fn unpinned_key_warning(&self, observed_key: &str) -> String {
    format!(
      "peer [{}] has no expected_key set; it advertised the key below. If you \
       trust this server, pin it by adding the following to your weverywhere.toml:\n\n{}",
      self.label(),
      self.to_pinned_toml(observed_key)
    )
  }
}

/// Render `s` as a TOML basic (double-quoted) string, escaping the characters
/// TOML requires. Inputs here (hostnames, IPs, ssh key strings) are normally
/// plain, but this keeps generated `[[peer]]` blocks valid regardless.
fn toml_basic_string(s: &str) -> String {
  let mut out = String::with_capacity(s.len() + 2);
  out.push('"');
  for c in s.chars() {
    match c {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      c => out.push(c),
    }
  }
  out.push('"');
  out
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct IdentityConfig {
  /// Human Name
  pub name: String,
  /// Private key file; TODO we will if/else on FIDO2/SmartCard/TPM data l8ter
  pub keyfile: std::path::PathBuf,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IdentityData {
  /// This is an untrusted value but is signed all the same; it may be ANY utf-8 set of characters up to 256 bytes long.
  pub human_name: String,

  /// seconds since 00:00 January 1, 1970 in UTC-0 time when this was generated and signed.
  /// If any system recieves an epoch_s claiming to be from the future it must be ignored or treated with the lowest possible trust.
  pub generated_at_utc0_epoch_s: u64,

  /// This allows for up to 16 hours of validity; we really want identity data to be re-signed regularly, and so
  /// are not using a larger integer to store the data.
  pub validity_s: u16,

  /// Up to 16 utf-8 bytes of description hint for how to interpret encoded_public_key
  pub encoded_public_key_fmt: String,
  /// The bytes used to create a verification key for all signatures from this identity. May be a utf-8 string or any other encoding format supported by weverywhere.
  pub encoded_public_key: Vec<u8>,

  /// Holds signature bytes in whatever format is hinted at by encoded_public_key_fmt
  /// The following fields are hashed in order: human_name, generated_at_utc0_epoch_s, validity_s, encoded_public_key_fmt, encoded_public_key
  pub signature: Vec<u8>,
}

impl IdentityData {
  pub async fn generate_from_config(config: &Config) -> DynResult<IdentityData> {
    let human_name = config.identity.name.clone();
    let validity_s = u16::MAX;
    let encoded_public_key_fmt = "ed25519".to_string(); // TODO dynamic keys once we support more than one format
    let encoded_public_key = config.identity.read_public_key_ed25519_pem_file().await.map_err(map_loc_err!())?.as_bytes().to_vec();
    let generated_at_utc0_epoch_s = sys_utils::epoch_seconds_now_utc0();

    let our_private_key = config.identity.read_private_key_ed25519_pem_file().await.map_err(map_loc_err!())?;

    let signature = IdentityData::sign_identity_data(
      &our_private_key,
      &human_name, &generated_at_utc0_epoch_s, &validity_s, &encoded_public_key_fmt, &encoded_public_key
    );

    Ok(IdentityData {
      human_name: human_name,
      generated_at_utc0_epoch_s: generated_at_utc0_epoch_s,
      validity_s: validity_s,
      encoded_public_key_fmt: encoded_public_key_fmt,
      encoded_public_key: encoded_public_key,
      signature: signature.to_vec(),
    })
  }

  pub fn sign_identity_data(signing_key: &ed25519_dalek::SigningKey,
                            human_name: &str, generated_at_utc0_epoch_s: &u64, validity_s: &u16,
                            encoded_public_key_fmt: &str, encoded_public_key: &[u8])
  -> ed25519_dalek::Signature {
    use ed25519_dalek::{Signature, Signer};
    use sha2::{Sha256, Digest};
    // Hash the message with SHA-256
    let mut hasher = sha2::Sha256::new();

    hasher.update(human_name.as_bytes());
    hasher.update(generated_at_utc0_epoch_s.to_le_bytes()); // Note: we use Little-Endian byte order for the signature. Arbitrary decision.
    hasher.update(validity_s.to_le_bytes());
    hasher.update(encoded_public_key_fmt.as_bytes());
    hasher.update(encoded_public_key);

    let hash = hasher.finalize();

    // Sign the hash
    signing_key.sign(&hash)
  }

  pub fn check_self_signature_b(&self) -> bool {
    match self.check_self_signature() {
      Ok(_) => true,
      Err(_e) => false,
    }
  }

  pub fn check_self_signature(&self) -> DynResult<()> {
    use ed25519_dalek::{Signature, Verifier};
    use sha2::{Sha256, Digest};
    // Hash the message with SHA-256
    let mut hasher = sha2::Sha256::new();

    hasher.update(self.human_name.as_bytes());
    hasher.update(self.generated_at_utc0_epoch_s.to_le_bytes()); // Note: we use Little-Endian byte order for the signature. Arbitrary decision.
    hasher.update(self.validity_s.to_le_bytes());
    hasher.update(self.encoded_public_key_fmt.as_bytes());
    hasher.update(&self.encoded_public_key);

    let hash = hasher.finalize();
    // #[allow(deprecated)] // Why is .as_slice() old? What replaces it?
    // let hash_64: [u8; 64] = hash.as_slice().try_into().map_err(map_loc_err!())?; // If the length fails, we error out.

    // Transform the encoded encoded_public_key into a key we can use to verify the signature
    let pub_key_32: [u8; 32] = self.encoded_public_key.as_slice().try_into().map_err(map_loc_err!())?; // If the length fails, we error out.
    let public_key = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_32).map_err(map_loc_err!())?;

    // Transform the serialized signature into a struct
    let signature = ed25519_dalek::Signature::from_slice(self.signature.as_slice()).map_err(map_loc_err!())?;

    // This will return with an Error if the signature does not match (see '?' at end)
    let _ = public_key.verify(&hash, &signature).map_err(map_loc_err!())?;

    // Signature is valid!

    Ok(())
  }

  pub fn check_claimed_signature_b(&self) -> bool {
    match self.check_claimed_signature() {
      Ok(_) => true,
      Err(_e) => false,
    }
  }

  pub fn check_claimed_signature(&self) -> DynResult<()> {
    std::unimplemented!()
  }

}



impl Config {
  pub async fn read_from_file(file: &std::path::Path) -> DynResult<Config> {
    let contents = tokio::fs::read_to_string(file).await?;
    let mut config: Config = toml::from_str(&contents)?;
    // Process all included files, applying them over the original file's data
    for include_struct in config.includes.clone().iter() {
      match glob::glob(&include_struct.path) {
        Ok(paths) => {
          for entry in paths {
            match entry {
              Ok(path) => {
                match process_config_override_file(&config, &path).await {
                  Ok(new_config) => {
                    config = new_config;
                  }
                  Err(e) => {
                    tracing::warn!("Error applying override file {:?} - {}", &path, e);
                  }
                }
              }
              Err(ref e) => tracing::warn!("Glob error when processing {:?} - {}", entry, e),
            }
          }
        }
        Err(e) => tracing::warn!("Invalid glob pattern while parsing {:?} - {}", file, e),
      }
    }
    Ok( config )
  }
}

async fn process_config_override_file(config: &Config, override_file_path: &std::path::Path) -> DynResult<Config> {
  let contents = tokio::fs::read_to_string(override_file_path).await?;

  let mut override_data: ConfigOpt = toml::from_str(&contents)?;
  override_data.includes = None; // I don't care, we're not recursively including other things -_-

  let config_o: ConfigOpt = config.clone().into_optioned();

  // This does not work - omerge does not descend to children, so we will need to do all minus the lowest level outselves. Ugh -_-
  // I had hoped to avoid this via the set of optionable::Optionable derives upstairs
  //let joined_o: ConfigOpt = serde_merge::omerge(config_o, override_data)?;

  let joined_o: ConfigOpt = ConfigOpt {
    identity: fancy_omerge(config_o.identity, override_data.identity)?,
    trusted: fancy_omerge_vec(config_o.trusted, override_data.trusted)?,
    startup_program: fancy_omerge_vec(config_o.startup_program, override_data.startup_program)?,
    includes: fancy_omerge_vec(config_o.includes, override_data.includes)?,
    peer: fancy_omerge_vec(config_o.peer, override_data.peer)?,
    limits: Some(LimitsOpt { // Oh god -_- at least it's read-once config data.
      trusted: Some(fancy_omerge(config_o.limits.clone().unwrap_or_else(|| Default::default()).trusted, override_data.limits.clone().unwrap_or_else(|| Default::default()).trusted)?.unwrap_or_else(|| Default::default())),
      untrusted: Some(fancy_omerge(config_o.limits.clone().unwrap_or_else(|| Default::default()).untrusted, override_data.limits.clone().unwrap_or_else(|| Default::default()).untrusted)?.unwrap_or_else(|| Default::default())),
    }),

    // TODO other top-level fields here
  };

  // We know this is safe, as the original Config had all values and serde_merge::omerge promises not to overwrite None values.
  Ok( Config::try_from_optioned(joined_o)? )
}

fn fancy_omerge<T>(f1: Option<T>, f2: Option<T>) -> DynResult<Option<T>>
where T: serde::Serialize + serde::de::DeserializeOwned
{
  match (f1, f2) {
    (Some(v1), None) => {
      Ok(Some(v1))
    }
    (Some(v1), Some(v2)) => {
      Ok( serde_merge::omerge(v1, v2)? )
    }
    (None, Some(v2)) => {
      Ok(Some(v2))
    }
    (None, None) => {
      Ok(None)
    }
  }
}

fn fancy_omerge_vec<T>(f1: Option<Vec<T>>, f2: Option<Vec<T>>) -> DynResult<Option<Vec<T>>>
where T: serde::Serialize + serde::de::DeserializeOwned
{
  match (f1, f2) {
    (Some(v1), None) => {
      Ok(Some(v1))
    }
    (Some(mut v1), Some(mut v2)) => {
      let mut combined_vec: Vec<T> = Vec::with_capacity(v1.len() + v2.len());
      combined_vec.append(&mut v1);
      combined_vec.append(&mut v2);
      Ok( Some(combined_vec) )
    }
    (None, Some(v2)) => {
      Ok(Some(v2))
    }
    (None, None) => {
      Ok(None)
    }
  }
}

impl IdentityConfig {
  pub async fn read_private_key_ed25519_pem_file(&self) -> DynResult<ed25519_dalek::SigningKey> {
    crypto_utils::read_private_key_ed25519_pem_file(&self.keyfile).await
  }
  pub async fn read_public_key_ed25519_pem_file(&self) -> DynResult<ed25519_dalek::VerifyingKey> {
    crypto_utils::read_public_key_ed25519_pem_file(&self.keyfile).await
  }
}


#[cfg(test)]
mod peer_tests {
  use super::*;

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
}
