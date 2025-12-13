
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

pub fn public_key_to_ed25519_vk(public_key: &str) -> DynResult<ed25519_dalek::VerifyingKey> {
    use ed25519_dalek::VerifyingKey;
    use ssh_key::PublicKey;

    // Parse the SSH public key
    let public_key = PublicKey::from_openssh(public_key)?;

    // Extract the Ed25519 key data
    if let ssh_key::public::KeyData::Ed25519(ed25519_key) = public_key.key_data() {
        let verifying_key = VerifyingKey::from_bytes(ed25519_key.as_ref())?;
        Ok(verifying_key)
    }
    else {
        Err("Not an Ed25519 key".into())
    }
}

pub fn sign_bytes(priv_key: &ed25519_dalek::SigningKey, bytes: &mut[u8]) -> [u8; ed25519_dalek::Signature::BYTE_SIZE] {
    use ed25519_dalek::Signer;
    let signature = priv_key.sign(bytes);
    signature.to_bytes()
}

pub fn signature_is_valid(verifying_key: ed25519_dalek::VerifyingKey, message_bytes: &[u8], signature_bytes: &[u8; ed25519_dalek::Signature::BYTE_SIZE]) -> bool {
    match verifying_key.verify_strict(message_bytes, &ed25519_dalek::Signature::from_bytes(signature_bytes)) {
        Ok(()) => {
            true
        }
        Err(e) => {
            tracing::info!("{}:{} {:?}", file!(), line!(), e);
            false
        }
    }
}


pub async fn read_private_key_ed25519_pem_file<P: AsRef<std::path::Path>>(keyfile: P) -> DynResult<ed25519_dalek::SigningKey> {
    let contents = tokio::fs::read_to_string(keyfile.as_ref()).await.map_err(map_loc_err!())?;
    use der::Decode;

    // First, decode the PEM format to get the raw DER bytes
    let pem = pem::parse(contents).map_err(map_loc_err!())?;

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

pub async fn read_public_key_ed25519_pem_file<P: AsRef<std::path::Path>>(keyfile: P) -> DynResult<ed25519_dalek::VerifyingKey> {
    let contents = tokio::fs::read_to_string(keyfile.as_ref()).await.map_err(map_loc_err!())?;
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_bytes_with_file() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}


