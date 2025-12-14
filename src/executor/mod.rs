
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
  running_programs: dashmap::DashMap<u64, std::sync::Arc<tokio::sync::RwLock<RunningProgram>> >,

  trusted_keys: dashmap::DashMap<String, ed25519_dalek::VerifyingKey>,

  event_loop_handle: tokio::task::JoinHandle<()>,

  startup_handle: tokio::task::JoinHandle<()>, // Used to confirm that any async start-up tasks have completed

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

  pub config: wasmtime::Config,
  pub engine: wasmtime::Engine,
  pub store: Option<wasmtime::Store<RPStoreData>>,

}

/// This structure participates in wasmtime function callbacks et al
pub struct RPStoreData {
  pub rp: std::sync::Arc<tokio::sync::RwLock<RunningProgram>>, // MUST point to the RunningProgram struct which holds the related Store<RPStoreData>
  pub instruction_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
  pub max_instructions: u64,
  pub wasi_p1_ctx: std::sync::Arc<tokio::sync::RwLock<wasmtime_wasi::p1::WasiP1Ctx>>,
}

unsafe impl Send for RPStoreData { } // TODO audit me
unsafe impl Sync for RPStoreData { } // TODO audit me


impl Executor {
  pub async fn new(config: &config::Config) -> std::sync::Arc<Executor> {
    let config = config.clone();
    std::sync::Arc::new_cyclic(|weak_ref| {
        // Upgrade inside the task
        let event_loop_weak_ref = weak_ref.clone();
        let event_loop_handle = tokio::spawn(async move {
            for _ in 0..10000 { // 5ms pauses, so in an error state where weak_ref is never populated we run for a max of 50s
              match event_loop_weak_ref.upgrade() {
                Some(arc) => {
                  let arc: std::sync::Arc<Executor> = arc; // Compiler forgot what type we were -_-
                  arc.event_loop().await;
                  break;
                }
                None => {
                  if crate::v_is_everything() {
                    tracing::info!("event_loop_weak_ref.upgrade() is None");
                  }
                  tokio::time::sleep(std::time::Duration::from_millis(5)).await; // Wait until we are constructed
                }
              }
            }
        });

        // We also assign the trusted key async; note that this means there is a very tiny amount of time
        // when we may not trust ourselves, so tasks being performed quickly should confirm that there is at least 1 trusted key
        // before assuming the trust store has been filled
        let initialization_work_weak_ref = weak_ref.clone();
        let our_identity_keyfile = config.identity.keyfile.clone();
        let startup_handle = tokio::spawn(async move {
          match crypto_utils::read_public_key_ed25519_pem_file(&our_identity_keyfile).await {
            Ok(our_pub_key) => {
              for _ in 0..10000 { // 5ms pauses, so in an error state where weak_ref is never populated we run for a max of 50s
                match initialization_work_weak_ref.upgrade() {
                  Some(arc) => {
                    let arc: std::sync::Arc<Executor> = arc; // Compiler forgot what type we were -_-
                    arc.add_trusted_key(
                      our_identity_keyfile.file_name().map(|fn_osstr| fn_osstr.to_string_lossy().to_string() ).unwrap_or_else(|| "SELF".to_string() ),
                      &our_pub_key
                    );
                    break;
                  }
                  None => {
                    if crate::v_is_everything() {
                      tracing::info!("initialization_work_weak_ref.upgrade() is None");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await; // Wait until we are constructed
                  }
                }
              }
            }
            Err(e) => {
              if crate::v_is_info() {
                  tracing::info!("Error reading our own public key: {}", e );
              }
            }
          }
        });

        // We now have a self-referrential Executor with some background logic going on, yay!
        Executor {
            config: config.clone(),

            next_pid: std::sync::atomic::AtomicU64::new(0),

            untrusted_allowed_instructions: std::sync::atomic::AtomicU64::new(16 * 1024),

            trusted_allowed_instructions: std::sync::atomic::AtomicU64::new(u64::MAX),

            // We use a high shard count (128) here on the expectation that many processes will be running in parallel,
            // and we want to enable lots of write capacity. This is a similar reason as why we have a large capacity up-front.
            running_programs: dashmap::DashMap::with_capacity_and_shard_amount(16 * 1024, 128),

            // We expect fewer writes to these during run-time, so we lower the shard amount to reduce overhead
            trusted_keys: dashmap::DashMap::with_capacity_and_shard_amount(256, 8),

            event_loop_handle: event_loop_handle,

            startup_handle: startup_handle,
        }
    })
  }

  pub async fn event_loop(&self) {
    loop {
      tokio::time::sleep(std::time::Duration::from_millis(250)).await;
      tracing::info!("event_loop tick!");
      // TODO
    }
  }

  pub fn add_trusted_key<S: AsRef<str>>(&self, name: S, key: &ed25519_dalek::VerifyingKey) {
    self.trusted_keys.insert(name.as_ref().into(), key.clone());
  }

  pub async fn begin_exec(&self, program: &ProgramData) -> DynResult<u64> {
    // Check 1: Is the program signature valid, given the identity it claims to have been signed by?
    match program.source.check_self_signature() {
      Ok(_) => { }
      Err(e) => {
        return Err(format!("The .source signature was invalid! {}", e).into());
      }
    }

    let mut is_trusted = false;

    for ref_m in self.trusted_keys.iter() {
      // let a: u8 = ref_m.key();
      // let b: u8 = ref_m.value();
      if program.source.encoded_public_key == ref_m.value().as_bytes() {
        is_trusted = true;
      }
    }

    self.create_pid(program, is_trusted).await
  }

  fn create_next_pid(&self) -> u64 {
    self.next_pid.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
  }

  async fn terminate_running_pid(&self, pid: u64) -> DynResult<()> {
    tracing::info!("TODO implement {}:{}", file!(), line!());
    Ok(())
  }

  async fn create_pid(&self, program: &ProgramData, program_is_trusted: bool) -> DynResult<u64> {
    // Allocate space in our PIDs; TODO check for wraparound and/or pre-existing stuff, terminate old when new PID is issued?
    let this_program_pid = self.create_next_pid();

    self.terminate_running_pid(this_program_pid).await?;

    let mut config = wasmtime::Config::new();
    config.consume_fuel(true); // Enable fuel tracking for instruction counting
    config.async_support(true); // Affects APIs available

    let engine = wasmtime::Engine::new(&config).map_err(map_loc_err!())?;

    let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
      .inherit_stdout()   // allow fd_write to stdout
      .inherit_stderr()   // allow fd_write to stderr
      // NOTE: do NOT call inherit_stdin()
      // NOTE: do NOT call preopen_dir()
      // NOTE: do NOT call inherit_args() unless you want argv
      // NOTE: do NOT call inherit_env() unless you want env vars
      //.build();
      .build_p1();


    // Construct a Running Program and begin executing it
    let arc_rp_data = std::sync::Arc::new(tokio::sync::RwLock::new(RunningProgram {
      data: program.clone(),
      config: config,
      engine: engine.clone(),
      store: None,
    }));

    let rps_store_data = RPStoreData {
      rp: arc_rp_data.clone(),
      instruction_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
      max_instructions: 16 * 1024, // todo
      wasi_p1_ctx: std::sync::Arc::new(tokio::sync::RwLock::new(wasi_ctx)),
    };

    { // Self-referential magic, now we can place the value in .store
      let mut write_lock = arc_rp_data.write().await;
      write_lock.store = Some(wasmtime::Store::new(&engine, rps_store_data));
    }

    self.running_programs.insert(this_program_pid, arc_rp_data);

    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;

    std::unimplemented!()
  }

  pub async fn wait_for_pid_exit(&self, pid: u64) -> DynResult<u32> {
    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
    std::unimplemented!();
    Ok(0)
  }

}


