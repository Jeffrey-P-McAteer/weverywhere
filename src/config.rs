
use crate::*;

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


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
  pub identity: IdentityConfig,
  pub includes: Vec<SingleInclude>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SingleInclude {
  pub path: String,
}


#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IdentityConfig {
  #[serde(rename = "name")]
  pub human_name: String,
  #[serde(rename = "key")]
  pub private_key_file: std::path::PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IdentityInline {
  pub human_name: String,
  pub encoded_public_key: Vec<u8>,
}



impl Config {
  pub async fn read_from_file(file: &std::path::Path) -> DynResult<Config> {
    let contents = tokio::fs::read_to_string(file).await?;
    let parsed_struct = toml::from_str(&contents)?;
    Ok( parsed_struct )
  }
}
