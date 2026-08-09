
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

  // NOTE: new variants MUST be appended, never reordered. serde_bare tags enum variants by their
  // positional index, so appending keeps ExecuteRequest=0 / Stdout=1 / Exit=2 stable and lets older
  // daemons keep parsing the messages they already understand.

  /// A structured result returned by a program as a CBOR *map* - the richer companion to
  /// BasicInsecureProgramStdout, for programs (like network-map) that return typed data rather than
  /// a blob of text. `cbor_data` is a CBOR-encoded map. `request_uuid` correlates the reply to a
  /// query session; as replies relay back up a multi-hop tree each node rewrites this to the UUID
  /// its own caller used, so the origin only ever sees its own request's UUID.
  BasicReturnMap {
    from_pid: u64,
    request_uuid: [u8; 16],
    cbor_data: Vec<u8>,
  },

  /// A structured result returned by a program as a CBOR *list*. See BasicReturnMap.
  BasicReturnList {
    from_pid: u64,
    request_uuid: [u8; 16],
    cbor_data: Vec<u8>,
  },
}





