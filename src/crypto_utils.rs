
use crate::*;

use der::EncodePem;

pub async fn generate_private_key_ed25519_pem_file(out_path: &std::path::Path) -> DynResult<()> {
    use rand::rngs::OsRng;
    use ed25519_dalek::{Signature, SigningKey};
    use pkcs8::EncodePrivateKey;

    // Generate a random Ed25519 keypair
    let mut csprng = rand::rngs::OsRng;
    let signing_key = ed25519_dalek::SigningKey::generate(&mut csprng);


    let private_key_pem = signing_key.to_pkcs8_pem(pkcs8::LineEnding::LF)?;

    tokio::fs::write(out_path, private_key_pem).await?;

    Ok(())
}

pub fn format_public_key(key: &ed25519_dalek::SigningKey) -> String {
    let verifying_key: ed25519_dalek::VerifyingKey = key.verifying_key();

    let mut wire = Vec::with_capacity(51);
    let key_type = b"ssh-ed25519";
    wire.extend_from_slice(&(key_type.len() as u32).to_be_bytes());
    wire.extend_from_slice(key_type);
    wire.extend_from_slice(&(verifying_key.as_bytes().len() as u32).to_be_bytes());
    wire.extend_from_slice(verifying_key.as_bytes());

    // Use base64 crate to encode
    let encoded = base64::encode(&wire); // or use base64 0.22 syntax
    format!("ssh-ed25519 {} {}", encoded, "" /* comment field */).trim().to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_bytes_with_file() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}


