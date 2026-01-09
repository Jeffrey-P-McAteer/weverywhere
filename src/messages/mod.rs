
use super::*;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum NetworkMessage {
  ExecuteRequest {
    program_data: executor::ProgramData,
  }
}





