
use crate::*;
use crate::args::*;

/**
 * Stores all data for the Executor.
 **/
pub struct Executor {

  next_pid: std::sync::atomic::AtomicU64,

  untrusted_allowed_instructions: std::sync::atomic::AtomicU64,

  trusted_allowed_instructions: std::sync::atomic::AtomicU64,

  /// Every program submited will get a unique number (PID) and RunningProgram entry here.
  running_programs: dashmap::DashMap<u64, RunningProgram>,

  trusted_keys: dashmap::DashMap<String, ed25519_dalek::VerifyingKey>,


}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProgramData {
  /// Used to determine the Controller / Client of this program, and the trust given to it by Executors / Servers.
  source: config::IdentityData,

  /// This is an untrusted value but is signed all the same; it may be ANY utf-8 set of characters up to 256 bytes long.
  human_name: String,

  /// The executable material.
  wasm_program_bytes: Vec<u8>,

  /// Holds signature bytes in whatever format is hinted at by source.encoded_public_key_fmt
  /// The following fields are hashed in order: all fields of source, human_name, wasm_program_bytes
  pub signature: Vec<u8>,

}

pub struct RunningProgram {
  pub data: ProgramData,

  pub engine: wasmtime::Engine,
  pub store: wasmtime::Store<RPStoreData>,

}

/// This structure participates in wasmtime function callbacks et al
pub struct RPStoreData {
  pub rp: Box<RunningProgram>, // MUST point to the RunningProgram struct which holds the related Store<RPStoreData>
  pub instruction_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
  pub max_instructions: u64,
  pub wasi_p1_ctx: wasmtime_wasi::p1::WasiP1Ctx,
}

impl Executor {
  pub fn new() -> Executor {
    Executor {
      next_pid: std::sync::atomic::AtomicU64::new(0),

      untrusted_allowed_instructions: std::sync::atomic::AtomicU64::new(16 * 1024),

      trusted_allowed_instructions: std::sync::atomic::AtomicU64::new(u64::MAX),

      // We use a high shard count (128) here on the expectation that many processes will be running in parallel,
      // and we want to enable lots of write capacity. This is a similar reason as why we have a large capacity up-front.
      running_programs: dashmap::DashMap::with_capacity_and_shard_amount(16 * 1024, 128),

      // We expect fewer writes to these during run-time, so we lower the shard amount to reduce overhead
      trusted_keys: dashmap::DashMap::with_capacity_and_shard_amount(256, 8),



    }
  }

  pub fn exec(&self, program: &ProgramData) -> DynResult<()> {
    std::unimplemented!()
    //Ok(())
  }

}




