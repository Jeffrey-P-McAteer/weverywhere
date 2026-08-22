
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

  /// This node's bounded log of messages received on the fabric (see [`MessageStore`]). Shared into
  /// every program execution so a "deliver" program can push what a "ui" program later reads. Wrapped
  /// in a std Mutex because the wasmtime host callbacks lock it only briefly.
  messages: std::sync::Arc<std::sync::Mutex<MessageStore>>,

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

/// CBOR integer keys inside a single message record returned by `host::messages_read`. Kept small so
/// the C chat program can hand-decode them; mirror any change in example-programs/chat.c.
pub mod message_keys {
  /// uint: monotonic sequence number assigned by the receiving host (readers track the max they've seen).
  pub const SEQ: i128 = 1;
  /// text: the sender's VERIFIED human name (from the signature-checked request source).
  pub const NAME: i128 = 2;
  /// byte string: the sender's VERIFIED identity pubkey.
  pub const PUBKEY: i128 = 3;
  /// uint: seconds since the UTC epoch when the receiving host recorded the message.
  pub const EPOCH_S: i128 = 4;
  /// byte string: the message body exactly as the sending program supplied it.
  pub const TEXT: i128 = 5;
}

/// One message recorded in a node's [`MessageStore`]. The identity fields are stamped by the host
/// from the request's already-verified `source`, so a program can never forge a sender's name/pubkey.
#[derive(Debug, Clone)]
pub struct StoredMessage {
  pub seq: u64,
  pub from_name: String,
  pub from_pubkey: Vec<u8>,
  pub text: Vec<u8>,
  pub epoch_s: u64,
}

/// A node's bounded, in-memory log of chat/messages received on the fabric. Lives on the [`Executor`]
/// and is shared (via Arc) into every program execution, so a short-lived "deliver" program can push
/// a message that a long-lived "ui" program on the same host later reads. Passive local state, like
/// the peers registry - NOT a wire protocol.
#[derive(Debug)]
pub struct MessageStore {
  next_seq: u64,
  msgs: std::collections::VecDeque<StoredMessage>,
  cap: usize,
  /// Recently-seen dedup keys (sender pubkey ++ sender-chosen message id), newest at the back. The
  /// same logical message reaches a node several times - multicast is emitted once per interface and
  /// also overlaps the unicast-to-peers path - so we drop repeats of a (pubkey,id) we've already
  /// recorded. A distinct message carries a fresh random id, so genuine repeats of the same text are
  /// still kept. Messages pushed with an empty id (non-chat callers) are never deduped.
  seen: std::collections::HashSet<Vec<u8>>,
  seen_order: std::collections::VecDeque<Vec<u8>>,
  seen_cap: usize,
}

impl MessageStore {
  pub fn new(cap: usize) -> MessageStore {
    MessageStore {
      next_seq: 1,
      msgs: std::collections::VecDeque::with_capacity(cap.min(1024)),
      cap,
      seen: std::collections::HashSet::new(),
      seen_order: std::collections::VecDeque::new(),
      seen_cap: 4096,
    }
  }
  /// Append a message with the host-stamped verified identity, deduplicated by `(from_pubkey, id)`.
  /// Returns the assigned sequence number, or 0 if this was a duplicate that was dropped. `id` is the
  /// sender-chosen per-message nonce; pass an empty slice to disable dedup for this push.
  pub fn push(&mut self, from_name: String, from_pubkey: Vec<u8>, id: &[u8], text: Vec<u8>, epoch_s: u64) -> u64 {
    if !id.is_empty() {
      let mut key = Vec::with_capacity(from_pubkey.len() + id.len());
      key.extend_from_slice(&from_pubkey);
      key.extend_from_slice(id);
      if !self.seen.insert(key.clone()) {
        return 0; // already seen this exact (sender, message) - a duplicate delivery
      }
      self.seen_order.push_back(key);
      while self.seen_order.len() > self.seen_cap {
        if let Some(old) = self.seen_order.pop_front() { self.seen.remove(&old); }
      }
    }
    let seq = self.next_seq;
    self.next_seq += 1;
    self.msgs.push_back(StoredMessage { seq, from_name, from_pubkey, text, epoch_s });
    while self.msgs.len() > self.cap { self.msgs.pop_front(); }
    seq
  }
  /// Messages with `seq > after_seq`, oldest first (clones, so the lock is released quickly).
  pub fn read_after(&self, after_seq: u64) -> Vec<StoredMessage> {
    self.msgs.iter().filter(|m| m.seq > after_seq).cloned().collect()
  }
}

/// Per-execution wiring the launching context supplies to [`Executor::begin_exec`]. Bundled into one
/// struct (rather than a growing parameter list) as the host surface expands. All fields are optional
/// so a plain batch run can pass `ExecOptions::default()`.
#[derive(Default)]
pub struct ExecOptions {
  /// This node's own address as the caller reached it, surfaced via `host::node_addr` (discovery).
  pub node_addr: Option<String>,
  /// Sink for `host::replicate` requests; `None` disables replication on this host.
  pub replicate_tx: Option<tokio::sync::mpsc::UnboundedSender<ReplicateRequest>>,
  /// Sink for `host::messages_send`: ready-to-transmit serialized `NetworkMessage::SignedFabricMessage`
  /// bytes the launcher fans out onto the fabric. `None` disables message sending on this host (the
  /// call then no-ops with -1). This is how a program broadcasts a signed message without re-shipping
  /// itself, unlike `replicate_tx`.
  pub fabric_send_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
  /// An attached interactive terminal, exposed via the `host::tty_*` imports; `None` = no terminal.
  pub tty: Option<std::sync::Arc<crate::tty::TtyHandle>>,
  /// Run without the instruction (fuel) cap. Required for long-lived interactive programs, which
  /// would otherwise trap once the fuel budget is exhausted. Only set this for trusted local launches.
  pub uncapped_fuel: bool,
}

/// Where a `host::replicate` copy should be sent. Kept as an enum so we can grow targets (a specific
/// peer, the local daemon, ...) without changing the host ABI (the guest passes a small integer).
#[derive(Debug, Clone)]
pub enum ReplicateScope {
  /// The whole multicast fabric (every group on every interface) plus every configured `[[peer]]`.
  Fabric,
}

/// A request, raised by a running program via `host::replicate`, to send a copy of ITSELF onto the
/// network with the given arguments. The program only declares intent + args; the host that launched
/// it (which holds the wasm bytes, our identity, and the network config) performs the actual send by
/// draining these off the channel. This keeps I/O out of the sandbox and is the general mechanism for
/// self-propagating / role-shifting programs (chat uses it to fan a "deliver" copy out to peers).
#[derive(Debug, Clone)]
pub struct ReplicateRequest {
  pub scope: ReplicateScope,
  pub arg_list: Vec<String>,
  pub arg_map: Vec<(String, String)>,
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

  // ---- Program arguments (general-purpose; how a program's behaviour is parameterised) ----
  /// Positional string arguments, read by the guest via `host::arg_get`. When a program replicates
  /// copies of itself onto the fabric it chooses these for each copy, so one program body can take on
  /// different roles as it spreads. Defaults to empty for a plain `run`.
  #[serde(default)]
  pub arg_list: Vec<String>,

  /// Named string arguments (key -> value), read by the guest via `host::arg_map_get`. Kept as an
  /// ordered Vec of pairs so the serde_bare encoding is deterministic. The chat program selects its
  /// role from `arg_map["mode"]`, for example. Defaults to empty for a plain `run`.
  #[serde(default)]
  pub arg_map: Vec<(String, String)>,
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
  arg_list: Vec<String>,
  arg_map: Vec<(String, String)>,
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
      arg_list: Vec::new(),
      arg_map: Vec::new(),
    }
  }
  /// Set the program's positional (`arg_list`) and named (`arg_map`) arguments in one call.
  pub fn set_args(mut self, arg_list: Vec<String>, arg_map: Vec<(String, String)>) -> Self {
    self.arg_list = arg_list;
    self.arg_map = arg_map;
    self
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
        arg_list: self.arg_list,
        arg_map: self.arg_map,
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
  /// The caller's VERIFIED human name (from the signature-checked source). Stamped onto messages the
  /// program pushes so a chat handle can't be forged. See `host::messages_push`.
  pub caller_name: String,
  /// This node's message store, shared from the [`Executor`] for `host::messages_push`/`messages_read`.
  pub messages: std::sync::Arc<std::sync::Mutex<MessageStore>>,
  /// Snapshot of this node's trusted identity pubkeys, for the general `host::trusts_key` query.
  pub trusted_pubkeys: Vec<Vec<u8>>,
  /// Where `host::replicate` deposits requests to send a copy of this program onward. The launcher
  /// that owns this program drains the channel and performs the send. `None` when the current host
  /// doesn't support replication (the call then no-ops with -1).
  pub replicate_tx: Option<tokio::sync::mpsc::UnboundedSender<ReplicateRequest>>,
  /// Where `host::messages_send` deposits ready-to-transmit signed message bytes for the launcher to
  /// fan out onto the fabric. `None` when this host can't send (call no-ops with -1).
  pub fabric_send_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
  /// This node's own signed identity, used as the `source` on messages sent via `host::messages_send`.
  /// `None` when the node has no identity key (message sending then no-ops).
  pub identity_data: Option<config::IdentityData>,
  /// An attached interactive terminal for the `host::tty_*` imports; `None` when not running in a UI
  /// context (the tty imports then report "no terminal").
  pub tty: Option<std::sync::Arc<crate::tty::TtyHandle>>,
  /// Hop distance from the origin (== inbound visited length; origin's direct responders are 1).
  pub depth: u32,
  /// This node's OWN socket address (`ip:port`) as it believes it is reachable by the caller, or
  /// None if it couldn't be determined. Surfaced via `host::node_addr` so a discovery program reports
  /// the real node address rather than the relay it was reached through. See [`Executor::begin_exec`].
  pub node_addr: Option<String>,
  /// Positional program arguments, snapshotted for the `host::arg_*` imports.
  pub arg_list: Vec<String>,
  /// Named program arguments (key -> value), snapshotted for the `host::arg_map_*` imports.
  pub arg_map: Vec<(String, String)>,
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

            // A few thousand messages is plenty for an interactive session; oldest are dropped.
            messages: std::sync::Arc::new(std::sync::Mutex::new(MessageStore::new(4096))),

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

  /// Append a signed fabric message (see [`messages::NetworkMessage::SignedFabricMessage`]) to this
  /// node's message store after the caller has verified both the sender's self-signature and the
  /// payload signature. The verified `from_name`/`from_pubkey` are stamped by the host (never guest
  /// data); `payload` is the opaque CBOR body the sender broadcast. Deduplicated by `(pubkey, id)`;
  /// returns the assigned sequence number, or 0 if it was a duplicate that was dropped.
  pub fn record_fabric_message(&self, from_name: String, from_pubkey: Vec<u8>, id: &[u8], payload: Vec<u8>, epoch_s: u64) -> u64 {
    match self.messages.lock() {
      Ok(mut s) => s.push(from_name, from_pubkey, id, payload, epoch_s),
      Err(_) => 0,
    }
  }

  pub async fn begin_exec(&self, program: &ProgramData, stdio_forwarder: executor::wasi_adapters::WasiStdioSimpleForwarder, opts: ExecOptions, return_slot: std::sync::Arc<std::sync::Mutex<ExecReturn>>) -> DynResult<u64> {
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

    self.create_pid(program, is_trusted, stdio_forwarder, opts, return_slot).await
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

  async fn create_pid(&self, program: &ProgramData, program_is_trusted: bool, mut stdio_forwarder: executor::wasi_adapters::WasiStdioSimpleForwarder, opts: ExecOptions, return_slot: std::sync::Arc<std::sync::Mutex<ExecReturn>>) -> DynResult<u64> {
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
    // Long-lived interactive programs (e.g. the chat UI) must run without the instruction cap or they
    // trap once fuel runs out; batch/fabric programs keep the cap. Fuel tracking is only enabled when
    // we intend to cap, since store.set_fuel requires it.
    config.consume_fuel(!opts.uncapped_fuel);
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
      caller_name: program.source.human_name.clone(),
      messages: self.messages.clone(),
      trusted_pubkeys: self.trusted_keys.iter().map(|kv| kv.value().as_bytes().to_vec()).collect(),
      replicate_tx: opts.replicate_tx,
      fabric_send_tx: opts.fabric_send_tx,
      identity_data: self.identity_data.clone(),
      tty: opts.tty,
      depth: program.visited.len() as u32,
      node_addr: opts.node_addr,
      arg_list: program.arg_list.clone(),
      arg_map: program.arg_map.clone(),
      return_slot: return_slot,
    };

    { // Self-referential magic, now we can place the value in .store
      let write_lock = arc_rp_data.read().await;
      let engine_read_lock = write_lock.engine.read().await;
      let mut store = wasmtime::Store::new(&engine_read_lock, rps_store_data);
      // Set initial fuel (roughly corresponds to instruction count) only when the engine is tracking
      // fuel; an uncapped interactive program has consume_fuel disabled and would reject set_fuel.
      if !opts.uncapped_fuel {
        store.set_fuel(128_000).map_err(map_loc_err!())?;
      }

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

      // host::arg_len() -> n. Number of positional program arguments (arg_list) available.
      linker.func_wrap_async(
          "host",
          "arg_len",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move { Ok(caller.data().arg_list.len() as i32) })
          },
      ).map_err(map_loc_err!())?;

      // host::arg_get(index, ptr, cap) -> bytes_written (or -1 if index is out of range). Writes the
      // positional argument at `index` into guest memory.
      linker.func_wrap_async(
          "host",
          "arg_get",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (index, ptr, cap): (i32, i32, i32)| {
            Box::new(async move {
              let val = caller.data().arg_list.get(index as usize).cloned();
              match val {
                Some(s) => write_guest_bytes(&mut caller, ptr, cap, s.as_bytes()),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::arg_map_len() -> n. Number of named arguments (arg_map key/value pairs).
      linker.func_wrap_async(
          "host",
          "arg_map_len",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move { Ok(caller.data().arg_map.len() as i32) })
          },
      ).map_err(map_loc_err!())?;

      // host::arg_map_key(index, ptr, cap) -> bytes_written (or -1 if out of range). Writes the KEY of
      // the named argument at `index`, so a program can enumerate all keys it was given.
      linker.func_wrap_async(
          "host",
          "arg_map_key",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (index, ptr, cap): (i32, i32, i32)| {
            Box::new(async move {
              let key = caller.data().arg_map.get(index as usize).map(|(k, _)| k.clone());
              match key {
                Some(s) => write_guest_bytes(&mut caller, ptr, cap, s.as_bytes()),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::arg_map_get(key_ptr, key_len, ptr, cap) -> bytes_written (or -1 if the key is absent).
      // Looks up a named argument by key and writes its value into guest memory.
      linker.func_wrap_async(
          "host",
          "arg_map_get",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (key_ptr, key_len, ptr, cap): (i32, i32, i32, i32)| {
            Box::new(async move {
              let key = read_guest_bytes(&mut caller, key_ptr, key_len)?;
              let key = String::from_utf8_lossy(&key).into_owned();
              let val = caller.data().arg_map.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
              match val {
                Some(s) => write_guest_bytes(&mut caller, ptr, cap, s.as_bytes()),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::messages_push(id_ptr, id_len, text_ptr, text_len) -> seq (0 if dropped as a duplicate).
      // Append a message to this node's message store. The host stamps the VERIFIED caller identity
      // (name + pubkey, from the signature-checked source) and the current time; the guest supplies a
      // per-message id (a random nonce) plus the body. The id deduplicates the several copies of one
      // message a node receives (multicast is emitted per interface and overlaps the unicast path);
      // pass an empty id to disable dedup. Open to all senders (chat is public) - trust is a separate
      // query via host::trusts_key.
      linker.func_wrap_async(
          "host",
          "messages_push",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (id_ptr, id_len, text_ptr, text_len): (i32, i32, i32, i32)| {
            Box::new(async move {
              let id = read_guest_bytes(&mut caller, id_ptr, id_len)?;
              let text = read_guest_bytes(&mut caller, text_ptr, text_len)?;
              let (name, pubkey, store) = {
                let d = caller.data();
                (d.caller_name.clone(), d.caller_pubkey.clone(), d.messages.clone())
              };
              let epoch = sys_utils::epoch_seconds_now_utc0();
              let seq = match store.lock() {
                Ok(mut s) => s.push(name, pubkey, &id, text, epoch),
                Err(_) => 0,
              };
              Ok(seq as i64)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::messages_read(after_seq, ptr, cap) -> bytes_written. Writes a CBOR array of message
      // records (keys per crate::executor::message_keys) with seq > after_seq into guest memory, newest
      // last. Not trust-gated: chat is public. If the full set won't fit in `cap`, the oldest matching
      // messages that fit are returned; the reader advances its high-water seq and gets the rest next
      // poll. Returns bytes written (0 when nothing new).
      linker.func_wrap_async(
          "host",
          "messages_read",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (after_seq, ptr, cap): (i64, i32, i32)| {
            Box::new(async move {
              let after = if after_seq < 0 { 0u64 } else { after_seq as u64 };
              let msgs = match caller.data().messages.lock() {
                Ok(s) => s.read_after(after),
                Err(_) => Vec::new(),
              };
              let cap = cap.max(0) as usize;
              let bytes = encode_messages_cbor(&msgs, cap);
              write_guest_bytes(&mut caller, ptr, cap as i32, &bytes)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::trusts_key(pubkey_ptr, pubkey_len) -> 1 if this node trusts the given identity pubkey,
      // else 0. The general form of host::trusts_me: lets a program that cares about trust evaluate any
      // sender's key (e.g. the pubkey on a message record) while trust-agnostic programs ignore it.
      linker.func_wrap_async(
          "host",
          "trusts_key",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (pubkey_ptr, pubkey_len): (i32, i32)| {
            Box::new(async move {
              let key = read_guest_bytes(&mut caller, pubkey_ptr, pubkey_len)?;
              let trusted = caller.data().trusted_pubkeys.iter().any(|k| k.as_slice() == key.as_slice());
              Ok(if trusted { 1i32 } else { 0i32 })
            })
          },
      ).map_err(map_loc_err!())?;

      // host::replicate(scope, args_ptr, args_len) -> 0 (queued) | -1 (no replication sink here).
      // Sends a copy of THIS program onto the network with the given arguments. `scope` selects the
      // target (0 = the whole fabric). `args` is a small CBOR map { 1: [list strings], 2: [flattened
      // k,v strings] } the guest builds; the launcher fills in the wasm bytes + our signed identity
      // and does the actual send. This is the general self-propagation / role-shift primitive.
      linker.func_wrap_async(
          "host",
          "replicate",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (scope, args_ptr, args_len): (i32, i32, i32)| {
            Box::new(async move {
              let args = read_guest_bytes(&mut caller, args_ptr, args_len)?;
              let (arg_list, arg_map) = decode_replicate_args(&args);
              let scope = match scope { _ => ReplicateScope::Fabric }; // only Fabric today; reserve others
              let req = ReplicateRequest { scope, arg_list, arg_map };
              match &caller.data().replicate_tx {
                Some(tx) => Ok(if tx.send(req).is_ok() { 0i32 } else { -1i32 }),
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::messages_send(cbor_ptr, cbor_len) -> 0 (queued) | negative on error. Broadcast a SIGNED
      // application message onto the fabric WITHOUT shipping a program (the lightweight alternative to
      // host::replicate for chat-style messaging). `cbor` MUST be a CBOR list or map - a bare scalar or
      // string is rejected (-2); wrap a lone string as a one-element list. The host mints a random dedup
      // nonce, signs SHA-256(nonce ++ cbor) with THIS node's identity key, wraps it in a
      // NetworkMessage::SignedFabricMessage carrying our self-signed identity, and hands the serialized
      // bytes to the launcher to fan out. Errors: -1 no send sink / no identity key, -2 payload not a
      // list/map, -3 serialization failed.
      linker.func_wrap_async(
          "host",
          "messages_send",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (cbor_ptr, cbor_len): (i32, i32)| {
            Box::new(async move {
              let payload = read_guest_bytes(&mut caller, cbor_ptr, cbor_len)?;
              // Enforce "no bare strings": the top-level CBOR item must be an array (major 4) or map
              // (major 5). Anything else (incl. text/byte strings and integers) is rejected.
              match payload.first().map(|b| b >> 5) {
                Some(4) | Some(5) => {}
                _ => return Ok(-2i32),
              }
              let (source, signing_key, tx) = {
                let d = caller.data();
                match (&d.identity_data, &d.signing_key, &d.fabric_send_tx) {
                  (Some(src), Some(key), Some(tx)) => (src.clone(), key.clone(), tx.clone()),
                  _ => return Ok(-1i32),
                }
              };
              let mut id = [0u8; 16];
              { use rand::RngCore; rand::rngs::OsRng.fill_bytes(&mut id); }
              let signature = config::IdentityData::sign_payload(&signing_key, &id, &payload).to_bytes().to_vec();
              let msg = messages::NetworkMessage::SignedFabricMessage {
                source, id: id.to_vec(), cbor_data: payload, signature,
              };
              match serde_bare::to_vec(&msg) {
                Ok(bytes) => Ok(if tx.send(bytes).is_ok() { 0i32 } else { -1i32 }),
                Err(_) => Ok(-3i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_available() -> 1 if an interactive terminal is attached to this execution, else 0.
      linker.func_wrap_async(
          "host",
          "tty_available",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move { Ok(if caller.data().tty.is_some() { 1i32 } else { 0i32 }) })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_size(ptr) -> bytes_written (4: cols u16 LE, rows u16 LE), or -1 if no terminal.
      linker.func_wrap_async(
          "host",
          "tty_size",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr,): (i32,)| {
            Box::new(async move {
              let tty = match caller.data().tty.clone() { Some(t) => t, None => return Ok(-1i32) };
              let (cols, rows) = tty.size();
              let mut b = [0u8; 4];
              b[0..2].copy_from_slice(&cols.to_le_bytes());
              b[2..4].copy_from_slice(&rows.to_le_bytes());
              write_guest_bytes(&mut caller, ptr, 4, &b)
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_next_event(ptr, cap, timeout_ms) -> bytes_written (encoded event, see crate::tty),
      // 0 if the timeout elapsed with no input, or -1 if there is no terminal. Blocking a whole thread
      // is avoided: this awaits, so other tasks (inbound messages, replication) keep running.
      linker.func_wrap_async(
          "host",
          "tty_next_event",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, cap, timeout_ms): (i32, i32, i32)| {
            Box::new(async move {
              let tty = match caller.data().tty.clone() { Some(t) => t, None => return Ok(-1i32) };
              let dur = std::time::Duration::from_millis(timeout_ms.max(0) as u64);
              match tty.next_event(dur).await {
                Some(ev) => {
                  let bytes = crate::tty::encode_event(&ev);
                  write_guest_bytes(&mut caller, ptr, cap, &bytes)
                }
                None => Ok(0i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_print(ptr, len) -> 0, or -1 if no terminal. Queues text at the cursor (present it
      // with host::tty_flush).
      linker.func_wrap_async(
          "host",
          "tty_print",
          move |mut caller: wasmtime::Caller<'_, RPStoreData>, (ptr, len): (i32, i32)| {
            Box::new(async move {
              let bytes = read_guest_bytes(&mut caller, ptr, len)?;
              match caller.data().tty.clone() {
                Some(tty) => { tty.print(&String::from_utf8_lossy(&bytes)); Ok(0i32) }
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_move(col, row) -> 0/-1. Move the cursor to a 0-based (col, row).
      linker.func_wrap_async(
          "host",
          "tty_move",
          move |caller: wasmtime::Caller<'_, RPStoreData>, (col, row): (i32, i32)| {
            Box::new(async move {
              match &caller.data().tty {
                Some(tty) => { tty.move_to(col.max(0) as u16, row.max(0) as u16); Ok(0i32) }
                None => Ok(-1i32),
              }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_clear() -> 0/-1. Clear the whole screen.
      linker.func_wrap_async(
          "host",
          "tty_clear",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move {
              match &caller.data().tty { Some(tty) => { tty.clear(); Ok(0i32) } None => Ok(-1i32) }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_style(fg, bg, attrs) -> 0/-1. Set colour (ANSI 0-15, -1 = default) and attributes
      // (bit0 bold, bit1 underline, bit2 reverse; 0 resets).
      linker.func_wrap_async(
          "host",
          "tty_style",
          move |caller: wasmtime::Caller<'_, RPStoreData>, (fg, bg, attrs): (i32, i32, i32)| {
            Box::new(async move {
              match &caller.data().tty { Some(tty) => { tty.style(fg, bg, attrs); Ok(0i32) } None => Ok(-1i32) }
            })
          },
      ).map_err(map_loc_err!())?;

      // host::tty_flush() -> 0/-1. Present everything queued since the last flush.
      linker.func_wrap_async(
          "host",
          "tty_flush",
          move |caller: wasmtime::Caller<'_, RPStoreData>, _unused: ()| {
            Box::new(async move {
              match &caller.data().tty { Some(tty) => { tty.flush(); Ok(0i32) } None => Ok(-1i32) }
            })
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

/// Encode `msgs` as a CBOR array of message-record maps (keys per [`message_keys`]), fitting within
/// `cap` bytes. If the whole set doesn't fit, the oldest messages that DO fit are returned (readers
/// advance their high-water seq and pick up the rest on the next poll). Returns `[]`-encoding bytes
/// when nothing fits or the list is empty.
fn encode_messages_cbor(msgs: &[StoredMessage], cap: usize) -> Vec<u8> {
  use serde_cbor::Value;
  let to_value = |m: &StoredMessage| {
    Value::Map(
      [
        (Value::Integer(message_keys::SEQ), Value::Integer(m.seq as i128)),
        (Value::Integer(message_keys::NAME), Value::Text(m.from_name.clone())),
        (Value::Integer(message_keys::PUBKEY), Value::Bytes(m.from_pubkey.clone())),
        (Value::Integer(message_keys::EPOCH_S), Value::Integer(m.epoch_s as i128)),
        (Value::Integer(message_keys::TEXT), Value::Bytes(m.text.clone())),
      ]
      .into_iter()
      .collect(),
    )
  };
  // Grow the returned prefix until adding one more record would overflow cap.
  let mut n = msgs.len();
  loop {
    let list: Vec<Value> = msgs[..n].iter().map(&to_value).collect();
    let encoded = serde_cbor::to_vec(&Value::Array(list)).unwrap_or_default();
    if encoded.len() <= cap || n == 0 {
      return encoded;
    }
    n -= 1;
  }
}

/// Decode the CBOR a guest passes to `host::replicate` into (arg_list, arg_map). Shape:
/// `{ 1: [text...], 2: [text...] }` where key 1 is the positional list and key 2 is the named map as
/// a flattened `[k0, v0, k1, v1, ...]` array. Anything missing/malformed yields empty vectors, so a
/// bad payload simply replicates with no args rather than trapping.
fn decode_replicate_args(bytes: &[u8]) -> (Vec<String>, Vec<(String, String)>) {
  use serde_cbor::Value;
  let map = match serde_cbor::from_slice::<Value>(bytes) {
    Ok(Value::Map(m)) => m,
    _ => return (Vec::new(), Vec::new()),
  };
  let as_texts = |v: Option<&Value>| -> Vec<String> {
    match v {
      Some(Value::Array(items)) => items
        .iter()
        .map(|it| match it {
          Value::Text(s) => s.clone(),
          Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
          _ => String::new(),
        })
        .collect(),
      _ => Vec::new(),
    }
  };
  let arg_list = as_texts(map.get(&Value::Integer(1)));
  let flat = as_texts(map.get(&Value::Integer(2)));
  let arg_map = flat.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect();
  (arg_list, arg_map)
}

/// Lowercase hex encoding, used to key peers by their public key and to render keys for programs.
// Shared with netmap + the daemon's security logs so identity hex is rendered identically everywhere.
use crate::crypto_utils::to_hex;

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


