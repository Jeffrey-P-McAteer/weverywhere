
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
    let pem = pem::parse(&contents)?;

    if crate::v_is_everything() {
      tracing::warn!("pem = {:?}", pem);
    }

    let pem_tag = pem.tag();
    let encoded_pem_contents = pem.contents();

    let pki = pkcs8::PrivateKeyInfo::try_from(encoded_pem_contents).map_err(|non_std_err| format!("{:?}", non_std_err) )?;

    if pki.private_key.len() != ed25519_dalek::SECRET_KEY_LENGTH {
      return Err(format!(
        "Error: Expected pki.private_key.len() ({}) to be exactly ed25519_dalek::SECRET_KEY_LENGTH ({}) bytes long. Refusing to parse unknown key material",
        pki.private_key.len(),
        ed25519_dalek::SECRET_KEY_LENGTH).into()
      )
    }

    let mut signing_key_bytes: [u8; ed25519_dalek::SECRET_KEY_LENGTH] = [0u8; ed25519_dalek::SECRET_KEY_LENGTH];
    for i in 0..ed25519_dalek::SECRET_KEY_LENGTH {
      signing_key_bytes[i] = pki.private_key[i]; // Safety: Already did length check upstairs
    }

    Ok( ed25519_dalek::SigningKey::from_bytes(&signing_key_bytes) )
  }
}
