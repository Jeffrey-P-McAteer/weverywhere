
use crate::*;

pub async fn read_private_key_file(file: &std::path::Path) -> DynResult<pem::Pem>  {
    let contents = tokio::fs::read_to_string(file).await?;
    let contents = contents.trim();
    Ok(pem::parse(&contents)?)
}

pub fn read_private_key(bytes: &[u8]) -> DynResult<()>  {

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


