
use crate::*;

use optionable::OptionableConvert;

#[allow(non_camel_case_types)]
#[derive(Default, Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "pki_type", content = "pki_details")]
pub enum PKI_Type {
  /// The security of your system is dependent on who can read this file - ensure you protect it!
  ED25519_File { private_key_file: std::path::PathBuf },

  /// The security of your system is dependent on your sources of entropy available.
  /// These will typically be fine (unless run in a VM where you do not trust the hypervisor).
  /// Note that your identity is temporary - when the process exits, your identity ceases to exist.
  #[default]
  ED25519_Random,

  /// The security of your system is dependent on your CPUs TPM, or the TPM implemented by your Hypervisor if you are a VM.
  TPM2,

  /// The security of your system is dependent on your FIDO2 token manufacturer and the physical FIDO2 USB peripheral you have plugged in;
  /// note that this will typically require a physical presence to perform cryptographic tasks.
  FIDO2,

  /// The security of your system is dependent on your Smartcard manufacturer and the physical smartcard you have plugged in;
  /// note that this will typically require a PIN code to perform cryptographic tasks. Hard-coding a PIN is an option
  /// with this library, but is not recommended. If a pin is not specified one must be entered on STDIN when prompted for the PIN,
  /// which will be re-used until the process exits or a cryptographic error occurs.
  SMARTCARD { device_name: Option<String>, pin: Option<String>, },
}


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

