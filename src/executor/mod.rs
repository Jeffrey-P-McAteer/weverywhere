
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

pub struct Program {

}

pub struct RunningProgram {

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
  pub fn exec(&self, program: &Program) -> DynResult<()> {
    std::unimplemented!()
    //Ok(())
  }
}




