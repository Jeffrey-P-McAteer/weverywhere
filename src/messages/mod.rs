
use super::*;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum NetworkMessage {
  ExecuteRequest {
    program_data: executor::ProgramData,
  },
  BasicInsecureProgramStdout {
    from_pid: u64,
    stdout_data: Vec<u8>,
  },
  BasicInsecureProgramExit {
    from_pid: u64,
    exit_code: u32, // match type in Executor::pid_last_exit_code values
  },
}





