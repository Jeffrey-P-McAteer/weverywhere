
use crate::*;
use crate::args::*;

/**
 * Stores all data for the Executor.
 **/
pub struct Executor {

  /// We need to keep a thread-safe copy of ourselves for use in passed-off threads -_-
  self_weakref: std::sync::Weak<Executor>,

  /// Stores host-set configuration such as which PKI identities are trusted
  ///
  config: config::Config,

  next_pid: std::sync::atomic::AtomicU64,

  untrusted_allowed_instructions: std::sync::atomic::AtomicU64,

  trusted_allowed_instructions: std::sync::atomic::AtomicU64,

  /// Every program submited will get a unique number (PID) and RunningProgram entry here.
  running_programs: dashmap::DashMap<u64, std::sync::Arc<tokio::sync::RwLock<RunningProgram>> >,
  pid_last_exit_code: dashmap::DashMap<u64, u32>,

  trusted_keys: dashmap::DashMap<String, ed25519_dalek::VerifyingKey>,

  /// Efficient OS primitive to wake up a ton of .await-ers.
  /// This one is fired every time a PID exits. The exit code may be found in pid_last_exit_code until a new process
  /// with the same PID is launched, at which point the code will be 0 until the process exits.
  pid_exit_signal: tokio::sync::Notify,
  running_programs_insert_signal: tokio::sync::Notify,

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

  pub program_is_trusted: bool,

  pub config: wasmtime::Config,
  pub engine: std::sync::Arc<tokio::sync::RwLock<wasmtime::Engine>>,
  pub store: tokio::sync::RwLock<Option<wasmtime::Store<RPStoreData>>>,
  pub module: tokio::sync::RwLock<Option<wasmtime::Module>>,
  pub linker: tokio::sync::RwLock<Option<wasmtime::Linker<RPStoreData>>>,

  /// For errors which occur after inserting into the running process map this will be set, and
  /// when set the program should not be considered running.
  pub spawn_error: tokio::sync::RwLock<Option<Box<dyn std::error::Error + Send + Sync>>>,
}

/// This structure participates in wasmtime function callbacks et al
pub struct RPStoreData {
  pub rp: std::sync::Arc<tokio::sync::RwLock<RunningProgram>>, // MUST point to the RunningProgram struct which holds the related Store<RPStoreData>
  pub instruction_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
  pub max_instructions: u64,
  //pub wasi_p1_ctx: std::sync::Arc<tokio::sync::RwLock<wasmtime_wasi::p1::WasiP1Ctx>>,
  pub wasi_p1_ctx: wasmtime_wasi::p1::WasiP1Ctx,
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
            self_weakref: weak_ref.clone(),

            config: config.clone(),

            next_pid: std::sync::atomic::AtomicU64::new(0),

            untrusted_allowed_instructions: std::sync::atomic::AtomicU64::new(16 * 1024),

            trusted_allowed_instructions: std::sync::atomic::AtomicU64::new(u64::MAX),

            // We use a high shard count (128) here on the expectation that many processes will be running in parallel,
            // and we want to enable lots of write capacity. This is a similar reason as why we have a large capacity up-front.
            running_programs: dashmap::DashMap::with_capacity_and_shard_amount(16 * 1024, 128),
            pid_last_exit_code: dashmap::DashMap::with_capacity_and_shard_amount(16 * 1024, 128),

            // We expect fewer writes to these during run-time, so we lower the shard amount to reduce overhead
            trusted_keys: dashmap::DashMap::with_capacity_and_shard_amount(256, 8),

            pid_exit_signal: tokio::sync::Notify::new(),
            running_programs_insert_signal: tokio::sync::Notify::new(),

            event_loop_handle: event_loop_handle,

            startup_handle: startup_handle,
        }
    })
  }

  pub async fn event_loop(&self) {
    loop {
      let new_running_program = self.running_programs_insert_signal.notified();
      if crate::v_is_everything() {
        tracing::info!("event_loop waiting on new_running_program.await;");
      }
      new_running_program.await;

      // Iterate all running programs, spawning ones which are setup to be run in their on Tokio tasks
      // suitable for running on any thread pool thread
      for rp in &self.running_programs {

      }

    }
  }

  pub async fn event_loop_run_program(&self) {

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
    if let Some(entry) = self.running_programs.get(&pid) {
      tracing::info!("TODO implement {}:{}", file!(), line!());
    }
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
      program_is_trusted: program_is_trusted,
      config: config,
      engine: std::sync::Arc::new(tokio::sync::RwLock::new(engine)),
      store: tokio::sync::RwLock::new(None),
      module: tokio::sync::RwLock::new(None),
      linker: tokio::sync::RwLock::new(None),
      spawn_error: tokio::sync::RwLock::new(None),
    }));

    let rps_store_data = RPStoreData {
      rp: arc_rp_data.clone(),
      instruction_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
      max_instructions: 16 * 1024, // todo
      //wasi_p1_ctx: std::sync::Arc::new(tokio::sync::RwLock::new(wasi_ctx)),
      wasi_p1_ctx: wasi_ctx,
    };

    { // Self-referential magic, now we can place the value in .store
      let write_lock = arc_rp_data.read().await;
      let engine_read_lock = write_lock.engine.read().await;
      let mut store = wasmtime::Store::new(&engine_read_lock, rps_store_data);
      // Set initial fuel (roughly corresponds to instruction count)
      store.set_fuel(128_000).map_err(map_loc_err!())?;

      *write_lock.store.write().await = Some(store);
    }

    // We also must link against all of OUR apis
    {
      let write_lock = arc_rp_data.write().await;
      let engine_read_lock = write_lock.engine.read().await;
      let mut linker = wasmtime::Linker::new(&engine_read_lock);

      wasmtime_wasi::p1::add_to_linker_async::<RPStoreData>(&mut linker, |linker_store_data| {
          &mut linker_store_data.wasi_p1_ctx
      }).map_err(map_loc_err!())?;

      // Bind a custom "host" module with a "print" function
      linker.func_wrap(
          "host",
          "pri  nt",
          |_caller: wasmtime::Caller<'_, RPStoreData>, ptr: i32, len: i32| -> wasmtime::Result<()> {
              tracing::info!("[WASM] Print called with ptr={}, len={}", ptr, len);
              Ok(())
          },
      ).map_err(map_loc_err!())?;

      // Add another custom function that returns a value
      linker.func_wrap(
          "env",
          "get_magic_number",
          |_caller: wasmtime::Caller<'_, RPStoreData>| -> wasmtime::Result<i32> {
              tracing::info!("[HOST] get_magic_number called, returning 42");
              Ok(42)
          },
      ).map_err(map_loc_err!())?;

      *write_lock.linker.write().await = Some(linker);
    }

    { // Assign to .module
      let write_lock = arc_rp_data.read().await;
      let engine_read_lock = write_lock.engine.read().await;
      let module = wasmtime::Module::new(&engine_read_lock, &program.wasm_program_bytes).map_err(map_loc_err!())?;

      *write_lock.module.write().await = Some(module);
    }

    // For now we'll just spawn main off in a new tokio task
    let running_arc_rp_data = arc_rp_data.clone();
    let runner_t_self_weakref = self.self_weakref.clone();
    tokio::spawn(async move {
      let write_lock = running_arc_rp_data.write().await;

      let mut linker_lock = write_lock.linker.write().await;
      let write_lock_module = write_lock.module.read().await;
      let mut write_lock_store = write_lock.store.write().await;

      let instance_res = linker_lock.as_mut().unwrap().instantiate_async(
        &mut write_lock_store.as_mut().unwrap(),
        &write_lock_module.as_ref().unwrap()
      ).await.map_err(map_loc_err!());

      match instance_res {
        Ok(instance) => {
          match instance.get_typed_func::<(), ()>(&mut write_lock_store.as_mut().unwrap(), "_start").map_err(map_loc_err!()) {
            Ok(main_func) => {
              match main_func.call_async(&mut write_lock_store.as_mut().unwrap(), ()).await.map_err(map_loc_err!()) {
                Ok(result) => {
                  // Set exit code
                  if let Some(self_arc) = runner_t_self_weakref.upgrade() {
                    self_arc.running_programs.remove(&this_program_pid);
                    self_arc.pid_last_exit_code.insert(this_program_pid, 0);
                    self_arc.pid_exit_signal.notify_waiters();
                  }
                  else {
                    if crate::v_is_everything() {
                      tracing::info!("runner_t_self_weakref.upgrade() was None! ({}:{})", file!(), line!());
                    }
                    // We can't remove the PID and we can't notify anyone. This is bad, TODO add resiliancy or something.
                  }
                }
                Err(e) => {
                  tracing::info!("{}", e);
                  {
                    *write_lock.spawn_error.write().await = Some(e.into());
                  }
                  if let Some(self_arc) = runner_t_self_weakref.upgrade() {
                    self_arc.running_programs.remove(&this_program_pid);
                    self_arc.pid_last_exit_code.insert(this_program_pid, 1);
                    self_arc.pid_exit_signal.notify_waiters();
                  }
                }
              }
            }
            Err(e) => {
              tracing::info!("{}", e);
              {
                *write_lock.spawn_error.write().await = Some(e.into());
              }
              if let Some(self_arc) = runner_t_self_weakref.upgrade() {
                  self_arc.running_programs.remove(&this_program_pid);
                  self_arc.pid_last_exit_code.insert(this_program_pid, 1);
                  self_arc.pid_exit_signal.notify_waiters();
                }
            }
          }
        }
        Err(e) => {
          tracing::info!("{}", e);
          {
            *write_lock.spawn_error.write().await = Some(e.into());
          }
          if let Some(self_arc) = runner_t_self_weakref.upgrade() {
            self_arc.running_programs.remove(&this_program_pid);
            self_arc.pid_last_exit_code.insert(this_program_pid, 1);
            self_arc.pid_exit_signal.notify_waiters();
          }
        }
      }

    });

    self.running_programs.insert(this_program_pid, arc_rp_data);

    self.running_programs_insert_signal.notify_waiters();

    Ok(this_program_pid)
  }

  pub async fn wait_for_pid_exit(&self, pid: u64) -> DynResult<u32> {
    loop {
      let pid_exit_notified = self.pid_exit_signal.notified();
      if crate::v_is_everything() {
        tracing::info!("wait_for_pid_exit is checking to see if {} has exited...", pid);
      }
      // If pid has been removed, has exited.
      if !self.running_programs.contains_key(&pid) {
        break;
      }

      if let Some(program_data) = self.running_programs.get(&pid) {
        if let Some(spawn_error) = program_data.read().await.spawn_error.write().await.take() { // Ownership: .take() places None back in if it was taken
          return Err(spawn_error); // And some caller gets the spawn error and is responsible for handling it
        }
      }

      pid_exit_notified.await;
    }
    Ok( self.pid_last_exit_code.get(&pid).map(|r| *r.value() ).unwrap_or(0) )
  }

}


