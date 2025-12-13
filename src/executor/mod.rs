
use crate::*;
use crate::args::*;

/**
 * Stores all data for the Executor.
 **/
pub struct Executor {

  /// Stores host-set configuration such as which PKI identities are trusted
  ///
  config: config::Config,

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

impl ProgramData {

}

pub struct ProgramDataBuilder {
  source: Option<config::IdentityData>,
  human_name: String,
  wasm_program_bytes: Vec<u8>,
  signature: Vec<u8>,
}

impl ProgramDataBuilder {
  pub fn new() -> ProgramDataBuilder {
    ProgramDataBuilder {
      source: None,
      human_name: "UNSET_NAME".to_string(),
      wasm_program_bytes: Vec::with_capacity(4096),
      signature: Vec::with_capacity(1024),
    }
  }
  pub fn set_source(mut self, source: &config::IdentityData) -> Self {
    self.source = Some(source.clone());
    self
  }
  pub fn set_human_name<T: AsRef<str>>(mut self, name: T) -> Self {
    self.human_name = name.as_ref().to_string();
    self
  }
  pub fn set_wasm_program_bytes<T: AsRef<[u8]>>(mut self, wasm_program_bytes: T) -> Self {
    self.wasm_program_bytes.clear();
    self.wasm_program_bytes.extend(wasm_program_bytes.as_ref());
    self
  }
  pub fn set_signature<T: AsRef<[u8]>>(mut self, signature: T) -> Self {
    self.signature.clear();
    self.signature.extend(signature.as_ref());
    self
  }
  pub fn build(self) -> DynResult<ProgramData> {
    if let Some(source) = self.source {
      Ok(ProgramData {
        source: source,
        human_name: self.human_name,
        wasm_program_bytes: self.wasm_program_bytes,
        signature: self.signature,
      })
    }
    else {
      Err("Error: source is None!".into())
    }
  }
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
  pub async fn new(config: &config::Config) -> Executor {
    let mut ex = Executor {
      config: config.clone(),

      next_pid: std::sync::atomic::AtomicU64::new(0),

      untrusted_allowed_instructions: std::sync::atomic::AtomicU64::new(16 * 1024),

      trusted_allowed_instructions: std::sync::atomic::AtomicU64::new(u64::MAX),

      // We use a high shard count (128) here on the expectation that many processes will be running in parallel,
      // and we want to enable lots of write capacity. This is a similar reason as why we have a large capacity up-front.
      running_programs: dashmap::DashMap::with_capacity_and_shard_amount(16 * 1024, 128),

      // We expect fewer writes to these during run-time, so we lower the shard amount to reduce overhead
      trusted_keys: dashmap::DashMap::with_capacity_and_shard_amount(256, 8),



    };
    match crypto_utils::read_public_key_ed25519_pem_file(&config.identity.keyfile).await {
      Ok(our_pub_key) => {
        ex.add_trusted_key(
          config.identity.keyfile.file_name().map(|fn_osstr| fn_osstr.to_string_lossy().to_string() ).unwrap_or_else(|| "SELF".to_string() ),
          &our_pub_key
        );
      }
      Err(e) => {
        if crate::v_is_info() {
            tracing::info!("Error reading our own public key: {}", e );
        }
      }
    }
    ex
  }

  pub fn add_trusted_key<S: AsRef<str>>(&mut self, name: S, key: &ed25519_dalek::VerifyingKey) {
    self.trusted_keys.insert(name.as_ref().into(), key.clone());
  }

  pub async fn exec(&self, program: &ProgramData) -> DynResult<()> {
    std::unimplemented!()
    //Ok(())
  }

}




