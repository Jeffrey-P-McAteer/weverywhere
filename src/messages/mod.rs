
use super::*;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ExecuteRequest {
  pub program_data: executor::ProgramData,
}






