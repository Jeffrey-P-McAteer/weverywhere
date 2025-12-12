
use super::*;

pub async fn configuration(args: &args::Args, style: ConfigStyle) -> DynResult<()> {
  match config::Config::read_from_file(&args.config).await {
    Ok(config_struct) => {
      tracing::info!("Configuration from {:?}", args.config);
      tracing::info!("{:#?}", config_struct);

      if style == ConfigStyle::CreateMissingKeys {
        if !tokio::fs::metadata(&config_struct.identity.keyfile).await.is_ok() {
          tracing::warn!("The file {:?} does not exist and a new identity will be generated.", &config_struct.identity.keyfile);
          crypto_utils::generate_private_key_ed25519_pem_file(&config_struct.identity.keyfile).await?;
        }
        else {
          tracing::warn!("The file {:?} already exists, refusing to overwrite with new key material!", &config_struct.identity.keyfile);
        }
      }

      let identity_key = config_struct.identity.read_private_key_ed25519_pem_file().await.map_err(map_loc_err!())?;
      tracing::info!("[ Identity Public Key ]");
      tracing::info!("{}", crypto_utils::format_public_key(&identity_key));

      for trusted in config_struct.trusted.iter() {
        match crypto_utils::public_key_to_ed25519_vk(&trusted.key) {
          Ok(vk) => tracing::info!("{:?}", vk),
          Err(e) => tracing::info!("{:?}", e),
        }
      }

    }
    Err(e) => {
      tracing::warn!("Failed to parse the config file {:?}", args.config);
      if let Some(parse_error) = e.downcast_ref::<toml::de::Error>() {
        tracing::warn!("> {}", parse_error.message());
      }
      else {
        tracing::warn!("{:?}", e);
      }
    }
  }
  Ok(())
}
