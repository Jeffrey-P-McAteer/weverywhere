
use crate::*;

use optionable::OptionableConvert;


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct Config {
  pub identity: IdentityConfig,
  pub includes: Vec<SingleInclude>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct SingleInclude {
  pub path: String,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, optionable::Optionable)]
#[optionable(derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize))]
pub struct IdentityConfig {
  /// Human Name
  pub name: String,
  /// Private key file; TODO we will if/else on FIDO2/SmartCard/TPM data l8ter
  pub key: std::path::PathBuf,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IdentityInline {
  pub human_name: String,
  pub encoded_public_key: Vec<u8>,
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
    includes: fancy_omerge(config_o.includes, override_data.includes)?,

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

impl IdentityConfig {
  pub async fn read_private_key_ed25519_pem_file(&self) -> DynResult<ed25519_dalek::SigningKey> {
    let contents = tokio::fs::read_to_string(&self.key).await?;
    use der::Decode;

    // First, decode the PEM format to get the raw DER bytes
    let pem = pem::parse(contents)?;

    // Verify this is a private key
    if pem.tag() != "PRIVATE KEY" {
        return Err(format!("Expected PRIVATE KEY label, got: {}", pem.tag()).into());
    }

    let der_bytes = pem.contents();

    // Parse the DER bytes as a PKCS#8 PrivateKeyInfo structure
    let private_key_info = pkcs8::PrivateKeyInfo::from_der(der_bytes).map_err(|non_std_err| format!("{:?}", non_std_err) )?;

    // Extract the raw private key bytes from the PKCS#8 structure
    let private_key_bytes = private_key_info.private_key;

    // The private key bytes for Ed25519 are wrapped in an OCTET STRING
    // We need to parse this inner OCTET STRING to get the actual 32 bytes
    let octet_string = der::asn1::OctetString::from_der(private_key_bytes).map_err(|non_std_err| format!("{:?}", non_std_err) )?;
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
}
