
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

    let signature = std::unimplemented!();

    Ok(IdentityData {
      human_name: human_name,
      generated_at_utc0_epoch_s: generated_at_utc0_epoch_s,
      validity_s: validity_s,
      encoded_public_key_fmt: encoded_public_key_fmt,
      encoded_public_key: encoded_public_key,
      signature: signature,
    })
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
    let contents = tokio::fs::read_to_string(&self.keyfile).await.map_err(map_loc_err!())?;
    use der::Decode;

    // First, decode the PEM format to get the raw DER bytes
    let pem = pem::parse(contents).map_err(map_loc_err!()).map_err(map_loc_err!())?;

    // Verify this is a private key
    if pem.tag() != "PRIVATE KEY" {
        return Err(format!("Expected PRIVATE KEY label, got: {}", pem.tag()).into());
    }

    let der_bytes = pem.contents();

    // Parse the DER bytes as a PKCS#8 PrivateKeyInfo structure
    let private_key_info = pkcs8::PrivateKeyInfo::from_der(der_bytes).map_err(|non_std_err| format!("{:?}", non_std_err) ).map_err(map_loc_err!())?;

    // Extract the raw private key bytes from the PKCS#8 structure
    let private_key_bytes = private_key_info.private_key;

    // The private key bytes for Ed25519 are wrapped in an OCTET STRING
    // We need to parse this inner OCTET STRING to get the actual 32 bytes
    let octet_string = der::asn1::OctetString::from_der(private_key_bytes).map_err(|non_std_err| format!("{:?}", non_std_err) ).map_err(map_loc_err!())?;
    let key_bytes = octet_string.as_bytes();

    // Ensure we have exactly 32 bytes
    if key_bytes.len() != 32 {
        return Err(format!("Expected 32 bytes for Ed25519 key, got {}", key_bytes.len()).into());
    }

    // Convert to array and create SigningKey
    let mut key_array = [0u8; 32];
    key_array.copy_from_slice(key_bytes);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&key_array);

    Ok(signing_key)

  }

  pub async fn read_public_key_ed25519_pem_file(&self) -> DynResult<ed25519_dalek::VerifyingKey> {
    let contents = tokio::fs::read_to_string(&self.keyfile).await.map_err(map_loc_err!())?;
    use der::Decode;
    // First, decode the PEM format to get the raw DER bytes
    let pem = pem::parse(contents).map_err(map_loc_err!())?;
    // Verify this is a private key
    if pem.tag() != "PRIVATE KEY" {
        return Err(format!("Expected PRIVATE KEY label, got: {}", pem.tag()).into());
    }
    let der_bytes = pem.contents();
    // Parse the DER bytes as a PKCS#8 PrivateKeyInfo structure
    let private_key_info = pkcs8::PrivateKeyInfo::from_der(der_bytes)
        .map_err(|non_std_err| format!("{:?}", non_std_err)).map_err(map_loc_err!())?;
    // Extract the raw private key bytes from the PKCS#8 structure
    let private_key_bytes = private_key_info.private_key;
    // The private key bytes for Ed25519 are wrapped in an OCTET STRING
    // We need to parse this inner OCTET STRING to get the actual 32 bytes
    let octet_string = der::asn1::OctetString::from_der(private_key_bytes)
        .map_err(|non_std_err| format!("{:?}", non_std_err)).map_err(map_loc_err!())?;
    let key_bytes = octet_string.as_bytes();
    // Ensure we have exactly 32 bytes
    if key_bytes.len() != 32 {
        return Err(format!("Expected 32 bytes for Ed25519 key, got {}", key_bytes.len()).into());
    }
    // Convert to array and create SigningKey
    let mut key_array = [0u8; 32];
    key_array.copy_from_slice(key_bytes);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&key_array);

    // Derive the public key from the private key
    let verifying_key = signing_key.verifying_key();

    Ok(verifying_key)
  }
}
