
use tokio::io::AsyncWriteExt;

use crate::*;
use crate::args::*;

pub mod wasi_adapters;

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

  /// This host's OS hostname, resolved once at construction. Exposed to WASI programs via the
  /// `host::hostname` import so discovery/observation programs can label the node they run on.
  hostname: String,

  /// Passively-observed registry of other identities seen on the fabric. We record the source of
  /// every inbound program request here (keyed by hex(pubkey)); this is NOT a discovery protocol,
  /// merely a memory of who has talked to us, exposed to WASI programs via `host::peer_*` so that
  /// a discovery program can report this node's neighbours. See readme "Network discovery".
  peers: dashmap::DashMap<String, PeerInfo>,

  /// Efficient OS primitive to wake up a ton of .await-ers.
  /// This one is fired every time a PID exits. The exit code may be found in pid_last_exit_code until a new process
  /// with the same PID is launched, at which point the code will be 0 until the process exits.
  pid_exit_signal: tokio::sync::Notify,
  running_programs_insert_signal: tokio::sync::Notify,

  event_loop_handle: tokio::task::JoinHandle<()>,

  startup_handle: tokio::task::JoinHandle<()>, // Used to confirm that any async start-up tasks have completed

  /// This node's own ed25519 signing key, loaded once from config.identity.keyfile. Used to sign the
  /// per-node attestation surfaced to programs via `host::signed_attestation`. None if the keyfile is
  /// missing (the node still serves, it just can't produce signed attestations).
  identity_signing_key: Option<ed25519_dalek::SigningKey>,

  /// Raw bytes of this node's identity public key (empty if no key). Doubles as this node's stable
  /// identity for the discovery visited-set.
  identity_pubkey: Vec<u8>,

  /// A freshly-signed identity for THIS node, reused as the `source` when the daemon forwards a
  /// discovery program onward (so each hop's `trusts_me` reflects the real parent). None if no key.
  identity_data: Option<config::IdentityData>,
}

/// A slot the discovery host functions write into and the serve loop reads after the program exits:
/// the node's own CBOR record (`host::return_map`) and the UUID it wants used for onward forwarding
/// (`host::set_forward_uuid`).
#[derive(Debug, Default, Clone)]
pub struct ExecReturn {
  pub map: Option<Vec<u8>>,
  pub forward_uuid: Option<[u8; 16]>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProgramData {
  /// Used to determine the Controller / Client of this program, and the trust given to it by Executors / Servers.
  pub source: config::IdentityData,

  /// This is an untrusted value but is signed all the same; it may be ANY utf-8 set of characters up to 256 bytes long.
  pub human_name: String,

  /// The executable material.
  pub wasm_program_bytes: Vec<u8>,

  /// Holds signature bytes in whatever format is hinted at by source.encoded_public_key_fmt
  /// The following fields are hashed in order: all fields of source, human_name, wasm_program_bytes
  pub signature: Vec<u8>,

  // ---- Recursive-discovery request context (defaults are inert for a normal `run`) ----
  /// Correlates replies to a query session. The origin picks a random UUID; each hop that forwards
  /// generates its own and rewrites replies back to its caller's UUID as they relay up.
  #[serde(default)]
  pub request_uuid: [u8; 16],

  /// How many more hops this program may be forwarded from the node that receives it. 0 = do not
  /// recurse (the node still executes and reports itself). Set from the trust-based budget in
  /// `crate::discovery`.
  #[serde(default)]
  pub depth_budget: u8,

  /// Identity public keys already on the path from the origin to here. A node skips any peer whose
  /// key is in this set (loop prevention) and appends its own key before forwarding.
  #[serde(default)]
  pub visited: Vec<Vec<u8>>,
}

impl ProgramData {

}

/// A single passively-observed neighbour on the fabric. Populated by [`Executor::note_peer`] from
/// the signed identity carried on inbound requests, and surfaced to WASI programs (one formatted
/// line per peer) through the `host::peer_report` import.
#[derive(Debug, Clone)]
pub struct PeerInfo {
  /// Untrusted self-declared human name from the peer's identity.
  pub human_name: String,
  /// Raw ed25519 public key bytes; doubles as this peer's stable identity.
  pub pubkey: Vec<u8>,
  /// The socket address we last received traffic from for this identity.
  pub last_addr: std::net::SocketAddr,
  /// Whether WE (this executor) trust the peer's key (present in our trusted-keys set).
  pub trusted: bool,
  /// Seconds since the UTC epoch when we last heard from this peer.
  pub last_seen_epoch_s: u64,
}

pub struct ProgramDataBuilder {
  source: Option<config::IdentityData>,
  human_name: String,
  wasm_program_bytes: Vec<u8>,
  signature: Vec<u8>,
  request_uuid: [u8; 16],
  depth_budget: u8,
  visited: Vec<Vec<u8>>,
}

impl ProgramDataBuilder {
  pub fn new() -> ProgramDataBuilder {
    ProgramDataBuilder {
      source: None,
      human_name: "UNSET_NAME".to_string(),
      wasm_program_bytes: Vec::with_capacity(4096),
      signature: Vec::with_capacity(1024),
      request_uuid: [0u8; 16],
      depth_budget: 0,
      visited: Vec::new(),
    }
  }
  /// Set the discovery request context (UUID + how many hops it may still be forwarded + the set of
  /// identity keys already visited). Inert for a normal `run` (defaults: zero UUID, depth 0, empty).
  pub fn set_request_context(mut self, request_uuid: [u8; 16], depth_budget: u8, visited: Vec<Vec<u8>>) -> Self {
    self.request_uuid = request_uuid;
    self.depth_budget = depth_budget;
    self.visited = visited;
    self
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
        request_uuid: self.request_uuid,
        depth_budget: self.depth_budget,
        visited: self.visited,
      })
    }
    else {
      Err("Error: source is None!".into())
    }
  }
}



pub struct RunningProgram {
  pub data: ProgramData,

  pub pid: u64,
  pub program_is_trusted: bool,

  pub config: wasmtime::Config,
  pub engine: std::sync::Arc<tokio::sync::RwLock<wasmtime::Engine>>,
  pub store: std::sync::Arc<tokio::sync::RwLock<Option<wasmtime::Store<RPStoreData>>>>,
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

  /// This host's name, snapshotted for the `host::hostname` import (see [`Executor::hostname`]).
  pub hostname: String,
  /// Pre-formatted peer lines snapshotted at spawn time for the `host::peer_report` import. Each is
  /// `name\taddr\ttrusted(0/1)\tpubkey_hex`. Snapshotting avoids sharing the live [`Executor`] into
  /// wasmtime callbacks and gives the program a stable view for the duration of its run.
  pub peer_reports: Vec<String>,

  // ---- Discovery per-exec context (snapshotted from the inbound ProgramData) ----
  /// This node's own signing key (for `host::signed_attestation`); None if we have no identity key.
  pub signing_key: Option<ed25519_dalek::SigningKey>,
  /// This node's identity pubkey bytes (goes into the attestation).
  pub our_pubkey: Vec<u8>,
  /// The caller's identity pubkey (this node's parent in the discovery tree).
  pub caller_pubkey: Vec<u8>,
  /// Hop distance from the origin (== inbound visited length; origin's direct responders are 1).
  pub depth: u32,
  /// This node's OWN socket address (`ip:port`) as it believes it is reachable by the caller, or
  /// None if it couldn't be determined. Surfaced via `host::node_addr` so a discovery program reports
  /// the real node address rather than the relay it was reached through. See [`Executor::begin_exec`].
  pub node_addr: Option<String>,
  /// Where discovery host functions deposit the node's CBOR record + onward-forwarding UUID for the
  /// serve loop to pick up after the program exits.
  pub return_slot: std::sync::Arc<std::sync::Mutex<ExecReturn>>,
}

unsafe impl Send for RPStoreData { } // TODO audit me
unsafe impl Sync for RPStoreData { } // TODO audit me


impl Executor {
  pub async fn new(config: &config::Config) -> std::sync::Arc<Executor> {
    let config = config.clone();
    // Resolve our hostname once, up front (new_cyclic's closure is synchronous).
    let hostname = get_hostname().await;
    // Load our identity key material once, up front, for signing node attestations and for signing
    // the `source` on forwarded discovery requests. Missing keyfile => None (node still serves).
    let identity_signing_key = crypto_utils::read_private_key_ed25519_pem_file(&config.identity.keyfile).await.ok();
    let identity_pubkey = identity_signing_key
      .as_ref()
      .map(|k| k.verifying_key().as_bytes().to_vec())
      .unwrap_or_default();
    let identity_data = config::IdentityData::generate_from_config(&config).await.ok();
    std::sync::Arc::new_cyclic(move |weak_ref| {
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

            hostname: hostname,
            // Peers accumulate slowly (one entry per distinct identity we hear from), so a small
            // shard count is plenty.
            peers: dashmap::DashMap::with_capacity_and_shard_amount(256, 8),

            pid_exit_signal: tokio::sync::Notify::new(),
            running_programs_insert_signal: tokio::sync::Notify::new(),

            event_loop_handle: event_loop_handle,

            startup_handle: startup_handle,

            identity_signing_key: identity_signing_key,
            identity_pubkey: identity_pubkey,
            identity_data: identity_data,
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

  /// True if `pubkey` (raw ed25519 bytes) is in our trusted-keys set. Used to pick the trusted vs
  /// untrusted forwarding depth for a peer.
  pub fn trusts_pubkey(&self, pubkey: &[u8]) -> bool {
    self.trusted_keys.iter().any(|kv| kv.value().as_bytes() == pubkey)
  }

  /// This node's identity public key bytes (empty if no keyfile). The discovery visited-set key.
  pub fn identity_pubkey(&self) -> Vec<u8> {
    self.identity_pubkey.clone()
  }

  /// A signed identity for THIS node to use as the `source` of forwarded discovery requests.
  pub fn identity_data(&self) -> Option<config::IdentityData> {
    self.identity_data.clone()
  }

  /// Snapshot of passively-observed neighbours as (last address, identity pubkey) - forwarding
  /// targets for recursive discovery, alongside the statically-configured `[[peer]]` list.
  pub fn observed_targets(&self) -> Vec<(std::net::SocketAddr, Vec<u8>)> {
    self.peers.iter().map(|kv| (kv.value().last_addr, kv.value().pubkey.clone())).collect()
  }

  /// Record (or refresh) a neighbour we just heard from on the fabric. This is intentionally
  /// passive observation, NOT a discovery protocol: we simply remember the signed identity that
  /// arrived on an inbound request so that later discovery *programs* can enumerate our neighbours
  /// via the `host::peer_*` imports. Keyed by hex(pubkey) so repeated contact updates in place.
  pub fn note_peer(&self, addr: std::net::SocketAddr, source: &config::IdentityData) {
    let trusted = self.trusted_keys.iter().any(|kv| source.encoded_public_key == kv.value().as_bytes());
    self.peers.insert(to_hex(&source.encoded_public_key), PeerInfo {
      human_name: source.human_name.clone(),
      pubkey: source.encoded_public_key.clone(),
      last_addr: addr,
      trusted: trusted,
      last_seen_epoch_s: sys_utils::epoch_seconds_now_utc0(),
    });
  }

  pub async fn begin_exec(&self, program: &ProgramData, stdio_forwarder: executor::wasi_adapters::WasiStdioSimpleForwarder, node_addr: Option<String>, return_slot: std::sync::Arc<std::sync::Mutex<ExecReturn>>) -> DynResult<u64> {
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

    self.create_pid(program, is_trusted, stdio_forwarder, node_addr, return_slot).await
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

  async fn create_pid(&self, program: &ProgramData, program_is_trusted: bool, mut stdio_forwarder: executor::wasi_adapters::WasiStdioSimpleForwarder, node_addr: Option<String>, return_slot: std::sync::Arc<std::sync::Mutex<ExecReturn>>) -> DynResult<u64> {
    // Allocate space in our PIDs; TODO check for wraparound and/or pre-existing stuff, terminate old when new PID is issued?
    let this_program_pid = self.create_next_pid();

    self.terminate_running_pid(this_program_pid).await?;

    stdio_forwarder.set_pid(this_program_pid); // Claim this PID - todo look at timeout stuff, we should not allow these to alias new processes

    // Snapshot host + peer facts for the WASI host:: imports so callbacks don't need the live Executor.
    let hostname_snapshot = self.hostname.clone();
    let peer_reports_snapshot: Vec<String> = self.peers.iter().map(|kv| {
      let p = kv.value();
      format!("{}\t{}\t{}\t{}", p.human_name, p.last_addr, if p.trusted { 1 } else { 0 }, to_hex(&p.pubkey))
    }).collect();

    let mut config = wasmtime::Config::new();
    config.consume_fuel(true); // Enable fuel tracking for instruction counting
    config.async_support(true); // Affects APIs available

    let engine = wasmtime::Engine::new(&config).map_err(map_loc_err!())?;

    // Construct a Running Program and begin executing it
    let arc_rp_data = std::sync::Arc::new(tokio::sync::RwLock::new(RunningProgram {
      data: program.clone(),
      pid: this_program_pid,
      program_is_trusted: program_is_trusted,
      config: config,
      engine: std::sync::Arc::new(tokio::sync::RwLock::new(engine)),
      store: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
      module: tokio::sync::RwLock::new(None),
      linker: tokio::sync::RwLock::new(None),
      spawn_error: tokio::sync::RwLock::new(None),
    }));

    let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
      //.inherit_stdout()   // allow fd_write to stdout
      //.inherit_stderr()   // allow fd_write to stderr
      // NOTE: do NOT call inherit_stdin()
      // NOTE: do NOT call preopen_dir()
      // NOTE: do NOT call inherit_args() unless you want argv
      // NOTE: do NOT call inherit_env() unless you want env vars
      //.build();
      .stdout( stdio_forwarder.clone() )
      .stderr( stdio_forwarder.clone() )
      .build_p1();

    let rps_store_data = RPStoreData {
      rp: arc_rp_data.clone(),
      instruction_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
      max_instructions: 16 * 1024, // todo
      //wasi_p1_ctx: std::sync::Arc::new(tokio::sync::RwLock::new(wasi_ctx)),
      wasi_p1_ctx: wasi_ctx,
      hostname: hostname_snapshot,
      peer_reports: peer_reports_snapshot,
      signing_key: self.identity_signing_key.clone(),
      our_pubkey: self.identity_pubkey.clone(),
      caller_pubkey: program.source.encoded_public_key.clone(),
      depth: program.visited.len() as u32,
      node_addr: node_addr,
      return_slot: return_slot,
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
      let host_print_stdio_forwarder_ref= stdio_forwarder.clone();
      linker.func_wrap_async(
          "host",
          "print",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, len) : (i32, i32) | {
            let mut owned_host_print_stdio_forwarder = host_print_stdio_forwarder_ref.clone();
              Box::new(async move {

                let our_pid = {
                  match caller.data().rp.try_read() {
                    Ok(rp_read_lock) => rp_read_lock.pid,
                    Err(e) => {
                      0
                    }
                  }
                };

                let memory = match caller.get_export("memory") {
                    Some(wasmtime::Extern::Memory(mem)) => mem,
                    _ => return Err(wasmtime::Trap::MemoryOutOfBounds.into()),
                };

                let data = memory
                    .data(&caller)
                    .get(ptr as usize..(ptr as usize + len as usize))
                    .ok_or_else(|| wasmtime::Trap::MemoryOutOfBounds)?;

                if let Err(e) = owned_host_print_stdio_forwarder.write_all(data).await {
                  tracing::info!("{}:{} {:?}", file!(), line!(), e);
                }

                // let msg = messages::NetworkMessage::BasicInsecureProgramStdout {
                //   from_pid: our_pid,
                //   stdout_data: data.to_vec(),
                // };
                // match serde_bare::to_vec(&msg) {
                //   Ok(msg_encoded) => {
                //     // Now send to client!

                //   }
                //   Err(e) => {
                //     tracing::info!("{}:{} {:?}", file!(), line!(), e);
                //   }
                // }

                Ok(())
            })
          },
      ).map_err(map_loc_err!())?;

      // Add another custom function that returns a value
      linker.func_wrap_async(
          "host",
          "trusts_me", // Clients call host::trusts_me() to determine if they are trusted. Catch me at a nomenclature class later.
          |caller: wasmtime::Caller<'_, RPStoreData>, unk: () | Box::new(async move{
            let program_is_trusted = {
              match caller.data().rp.try_read() {
                Ok(rp_read_lock) => rp_read_lock.program_is_trusted,
                Err(e) => {
                  false
                }
              }
            };
            if program_is_trusted {
              Ok(1 as i32) // True == 1
            }
            else {
              Ok(0 as i32) // False == 0
            }
          }),
      ).map_err(map_loc_err!())?;

      // host::hostname(ptr, cap) -> bytes_written. Copies this executor's hostname into guest
      // memory (up to `cap` bytes) and returns how many bytes were written. Lets a program label
      // the node it is currently executing on.
      linker.func_wrap_async(
          "host",
          "hostname",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, cap): (i32, i32)| {
            Box::new(async move {
              let bytes = caller.data().hostname.clone().into_bytes();
              write_guest_bytes(&mut caller, ptr, cap, &bytes)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::random(ptr, len) -> bytes_written. Fill [ptr, ptr+len) in guest memory with
      // cryptographically secure random bytes from the OS CSPRNG. Programs use this to seed things
      // like the per-hop request UUID in network-map (a WASI module has no entropy source of its
      // own). Returns the number of bytes written (== len unless the buffer overruns guest memory).
      linker.func_wrap_async(
          "host",
          "random",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, len): (i32, i32)| {
            Box::new(async move {
              use rand::RngCore;
              let n = len.max(0) as usize;
              let mut buf = vec![0u8; n];
              rand::rngs::OsRng.fill_bytes(&mut buf);
              write_guest_bytes(&mut caller, ptr, len, &buf)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::peer_count() -> n. Number of neighbours this executor has passively observed and can
      // report via host::peer_report.
      linker.func_wrap_async(
          "host",
          "peer_count",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move {
              Ok(caller.data().peer_reports.len() as i32)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::peer_report(index, ptr, cap) -> bytes_written (or -1 if index is out of range).
      // Writes one tab-separated record `name\taddr\ttrusted(0/1)\tpubkey_hex` for peer `index`.
      linker.func_wrap_async(
          "host",
          "peer_report",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (index, ptr, cap): (i32, i32, i32)| {
            Box::new(async move {
              let line = caller.data().peer_reports.get(index as usize).cloned();
              match line {
                Some(s) => write_guest_bytes(&mut caller, ptr, cap, s.as_bytes()),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::caller_pubkey(ptr, cap) -> n. Writes the identity pubkey of the caller that sent us
      // this program (this node's parent in the discovery tree) into guest memory.
      linker.func_wrap_async(
          "host",
          "caller_pubkey",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, cap): (i32, i32)| {
            Box::new(async move {
              let bytes = caller.data().caller_pubkey.clone();
              write_guest_bytes(&mut caller, ptr, cap, &bytes)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::depth() -> i32. This node's hop distance from the origin (origin's direct responders
      // report depth 1). Lets the program annotate its record so the client can render the tree.
      linker.func_wrap_async(
          "host",
          "depth",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move { Ok(caller.data().depth as i32) })
          },
      ).map_err(map_loc_err!())?;

      // host::node_addr(ptr, cap) -> bytes_written (or -1 if this node couldn't determine its own
      // address). Writes this node's OWN socket address (`ip:port`) - the address it believes the
      // caller reached it on - into guest memory. Discovery programs put this in their record so the
      // client shows the real node address instead of the relay it was reached through.
      linker.func_wrap_async(
          "host",
          "node_addr",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, cap): (i32, i32)| {
            Box::new(async move {
              match caller.data().node_addr.clone() {
                Some(s) => write_guest_bytes(&mut caller, ptr, cap, s.as_bytes()),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::signed_attestation(ptr, cap) -> bytes_written (or -1 if this node has no identity key).
      // Builds a CBOR attestation {hostname, pubkey, epoch, signature} signed by this node's identity
      // key over the canonical bytes in crate::discovery. This is what proves to the caller that the
      // record came from this node and no other.
      linker.func_wrap_async(
          "host",
          "signed_attestation",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, cap): (i32, i32)| {
            Box::new(async move {
              let cbor = {
                let d = caller.data();
                match &d.signing_key {
                  Some(sk) => {
                    use ed25519_dalek::Signer;
                    let epoch = sys_utils::epoch_seconds_now_utc0();
                    let msg = crate::discovery::attestation_signing_bytes(&d.hostname, &d.our_pubkey, epoch);
                    let sig = sk.sign(&msg).to_bytes().to_vec();
                    crate::discovery::build_attestation_cbor(&d.hostname, &d.our_pubkey, epoch, &sig).ok()
                  }
                  None => None,
                }
              };
              match cbor {
                Some(bytes) => write_guest_bytes(&mut caller, ptr, cap, &bytes),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::return_map(ptr, len) -> 0. Hands the program's CBOR node-record to the daemon, which
      // sends it to the caller as a BasicReturnMap after the program exits. (The richer companion to
      // host::print, which only forwards raw stdout.)
      linker.func_wrap_async(
          "host",
          "return_map",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, len): (i32, i32)| {
            Box::new(async move {
              let bytes = read_guest_bytes(&mut caller, ptr, len)?;
              if let Ok(mut slot) = caller.data().return_slot.lock() {
                slot.map = Some(bytes);
              }
              Ok(0i32)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::set_forward_uuid(ptr, len). Tells the daemon which UUID to stamp on the sub-requests it
      // forwards to this node's peers. The program seeds this from host::random, so a fresh random
      // UUID is generated per hop (and rewritten back to the caller's UUID as replies relay up).
      linker.func_wrap_async(
          "host",
          "set_forward_uuid",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, len): (i32, i32)| {
            Box::new(async move {
              let bytes = read_guest_bytes(&mut caller, ptr, len)?;
              if bytes.len() == 16 {
                let mut uuid = [0u8; 16];
                uuid.copy_from_slice(&bytes);
                if let Ok(mut slot) = caller.data().return_slot.lock() {
                  slot.forward_uuid = Some(uuid);
                }
              }
              Ok(0i32)
            })
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


      let instance_res = {
        let write_lock = running_arc_rp_data.write().await;

        let mut linker_lock = write_lock.linker.write().await;
        let write_lock_module = write_lock.module.read().await;
        let mut write_lock_store = write_lock.store.write().await;

        linker_lock.as_mut().unwrap().instantiate_async(
          &mut write_lock_store.as_mut().unwrap(),
          &write_lock_module.as_ref().unwrap()
        ).await.map_err(map_loc_err!())
      };

      match instance_res {
        Ok(instance) => {
          let store_rw = {
            let wg = running_arc_rp_data.write().await;
            wg.store.clone()
          };
          let mut write_lock_store = store_rw.write().await;
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
                    *running_arc_rp_data.write().await.spawn_error.write().await = Some(e.into());
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
                *running_arc_rp_data.write().await.spawn_error.write().await = Some(e.into());
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
            *running_arc_rp_data.write().await.spawn_error.write().await = Some(e.into());
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

/// Copy `src` (clamped to `cap` bytes) into a running program's linear memory at `ptr`, returning
/// the number of bytes written. Shared by the `host::hostname` / `host::peer_report` imports. Traps
/// if the module has no `memory` export or the destination range is out of bounds.
fn write_guest_bytes(caller: &mut wasmtime::Caller<'_, RPStoreData>, ptr: i32, cap: i32, src: &[u8]) -> wasmtime::Result<i32> {
  let memory = match caller.get_export("memory") {
    Some(wasmtime::Extern::Memory(mem)) => mem,
    _ => return Err(wasmtime::Trap::MemoryOutOfBounds.into()),
  };
  let cap = cap.max(0) as usize;
  let n = src.len().min(cap);
  let start = ptr.max(0) as usize;
  let dst = memory
    .data_mut(&mut *caller)
    .get_mut(start..start + n)
    .ok_or(wasmtime::Trap::MemoryOutOfBounds)?;
  dst.copy_from_slice(&src[..n]);
  Ok(n as i32)
}

/// Read `len` bytes from a running program's linear memory at `ptr`. Shared by the `host::return_map`
/// / `host::set_forward_uuid` imports. Traps if the module has no `memory` export or the range is out
/// of bounds.
fn read_guest_bytes(caller: &mut wasmtime::Caller<'_, RPStoreData>, ptr: i32, len: i32) -> wasmtime::Result<Vec<u8>> {
  let memory = match caller.get_export("memory") {
    Some(wasmtime::Extern::Memory(mem)) => mem,
    _ => return Err(wasmtime::Trap::MemoryOutOfBounds.into()),
  };
  let start = ptr.max(0) as usize;
  let n = len.max(0) as usize;
  let data = memory
    .data(&*caller)
    .get(start..start + n)
    .ok_or(wasmtime::Trap::MemoryOutOfBounds)?;
  Ok(data.to_vec())
}

/// Lowercase hex encoding, used to key peers by their public key and to render keys for programs.
fn to_hex(bytes: &[u8]) -> String {
  let mut s = String::with_capacity(bytes.len() * 2);
  for b in bytes {
    s.push_str(&format!("{:02x}", b));
  }
  s
}

/// Best-effort OS hostname, resolved by shelling out to the `hostname` command (present on Linux,
/// macOS, and Windows). Falls back to "unknown-host" so a missing tool never breaks the executor.
async fn get_hostname() -> String {
  match tokio::process::Command::new("hostname").output().await {
    Ok(out) if out.status.success() => {
      let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
      if name.is_empty() { "unknown-host".to_string() } else { name }
    }
    _ => "unknown-host".to_string(),
  }
}


