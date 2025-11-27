

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Identity {
  pub human_name: String,
  pub public_key: Vec<u8>,
}


#[derive(serde::Deserialize, serde::Serialize)]
pub struct ExecuteRequest {
  pub message: String,
  pub misc: u32,
}






