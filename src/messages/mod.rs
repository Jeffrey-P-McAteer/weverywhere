
use super::*;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum NetworkMessage {
  ExecuteRequest {
    program_data: executor::ProgramData,
  },
  ProgramStdout {
    from_pid: u64,
    stdout_data: Vec<u8>,
  }
}





