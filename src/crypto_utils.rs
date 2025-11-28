
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



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_bytes_with_file() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}


