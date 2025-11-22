

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
  pub includes: Vec<SingleInclude>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SingleInclude {
  pub path: String,
}



