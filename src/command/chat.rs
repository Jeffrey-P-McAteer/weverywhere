use super::*;

/// The stem of the bundled chat program (see example-programs/embedded.list + chat.c).
const EMBEDDED_CHAT_NAME: &str = "chat";

/// `weverywhere chat`: a self-contained interactive chat host. One process that (1) listens on the
/// fabric like `serve`, so inbound "deliver" copies of the chat program push messages into a shared
/// store, and (2) attaches the terminal and runs the chat program in `ui` mode, which draws the
/// transcript, reads the keyboard, and fans each typed line out to the fabric via `host::replicate`.
///
/// The message-passing is entirely the WASI program + host primitives (args, tty, messages,
/// replicate); this command is just the launcher that wires a terminal and a fabric listener to one
/// executor. Run this INSTEAD of `serve` on a machine (both bind the same port).
pub async fn chat(
  args: &args::Args,
  program: Option<std::path::PathBuf>,
  multicast_groups: args::MulticastAddressVec,
  port: u16,
) -> DynResult<()> {
  let local_config = config::Config::read_from_file(&args.config_path()).await.map_err(map_loc_err!())?;
  let source = config::IdentityData::generate_from_config(&local_config).await.map_err(map_loc_err!())?;
  let (wasm_bytes, program_label) = resolve_chat_program(program).await?;
  if crate::v_is_info() {
    tracing::info!("[ chat ] program: {}", program_label);
  }

  let local_config_arc = std::sync::Arc::new(local_config.clone());
  let executor = executor::Executor::new(&local_config).await;

  // Attach the terminal first. If there's no usable TTY (piped stdio, etc.) there's nothing to drive,
  // so fail early with guidance rather than starting listeners. The guard restores the terminal on
  // drop - including if the guest traps - so it must outlive the UI program below.
  let (tty_handle, _tty_guard) = match crate::tty::attach() {
    Ok(t) => t,
    Err(e) => return Err(format!(
      "chat needs an interactive terminal but couldn't attach to one ({e}). Run it directly in a shell."
    ).into()),
  };

  // Fabric listeners on the SAME executor: inbound `deliver` copies push into the executor's message
  // store, which the UI program reads. Left running in the background (we don't join the set).
  let mut listeners = serve::spawn_listeners(executor.clone(), local_config_arc.clone(), multicast_groups.clone(), port);

  // host::replicate sink: the UI deposits `deliver` copies here; this task broadcasts each onto the
  // fabric (multicast + peers), carrying the same chat wasm signed by our identity.
  let (replicate_tx, mut replicate_rx) = tokio::sync::mpsc::unbounded_channel::<executor::ReplicateRequest>();
  let drain = {
    let wasm_bytes = wasm_bytes.clone();
    let source = source.clone();
    let groups: Vec<std::net::IpAddr> = multicast_groups.iter().copied().collect();
    let peers = local_config.peer.clone();
    tokio::spawn(async move {
      while let Some(req) = replicate_rx.recv().await {
        match req.scope {
          executor::ReplicateScope::Fabric => {
            if let Err(e) = run::broadcast_program_to_fabric(
              &wasm_bytes, &source, EMBEDDED_CHAT_NAME, req.arg_list, req.arg_map, &groups, port, &peers,
            ).await {
              tracing::warn!("[ chat ] replicate failed: {:?}", e);
            }
          }
        }
      }
    })
  };

  // Launch the long-lived UI instance: uncapped fuel (it loops), with the terminal + replication sink.
  let pd = executor::ProgramDataBuilder::new()
    .set_human_name(EMBEDDED_CHAT_NAME)
    .set_wasm_program_bytes(&wasm_bytes)
    .set_source(&source)
    .set_args(Vec::new(), vec![("mode".to_string(), "ui".to_string())])
    .build().map_err(map_loc_err!())?;
  let return_slot = std::sync::Arc::new(std::sync::Mutex::new(executor::ExecReturn::default()));
  let opts = executor::ExecOptions {
    tty: Some(tty_handle),
    replicate_tx: Some(replicate_tx),
    uncapped_fuel: true,
    ..Default::default()
  };
  let pid = executor
    .begin_exec(&pd, executor::wasi_adapters::WasiStdioSimpleForwarder::new_nop(), opts, return_slot)
    .await
    .map_err(map_loc_err!())?;

  // Block until the user quits the UI (Ctrl-C / Esc). Then stop background work; dropping _tty_guard
  // restores the terminal.
  let _ = executor.wait_for_pid_exit(pid).await;
  listeners.abort_all();
  drain.abort();
  Ok(())
}

/// Resolve which chat program bytes to run: explicit `--program`, else the embedded program, else the
/// on-disk compiled example (dev fallback). Mirrors netmap's resolution so a moved binary is
/// self-contained.
async fn resolve_chat_program(program: Option<std::path::PathBuf>) -> DynResult<(Vec<u8>, String)> {
  if let Some(path) = program {
    match tokio::fs::read(&path).await {
      Ok(bytes) => return Ok((bytes, format!("override file {}", path.display()))),
      Err(e) => return Err(format!("Could not read --program {:?} ({})", path, e).into()),
    }
  }
  if let Some(bytes) = crate::embedded_programs::get(EMBEDDED_CHAT_NAME) {
    return Ok((bytes.to_vec(), format!("embedded '{EMBEDDED_CHAT_NAME}'")));
  }
  let fallback = std::path::PathBuf::from("target").join("example-programs").join(format!("{EMBEDDED_CHAT_NAME}.wasm"));
  match tokio::fs::read(&fallback).await {
    Ok(bytes) => Ok((bytes, format!("on-disk {}", fallback.display()))),
    Err(e) => Err(format!(
      "No chat program available: this binary has no embedded '{EMBEDDED_CHAT_NAME}' and {:?} could \
       not be read ({}). Build with zig to embed it, run `uv run scripts/compile-example-programs.py`, \
       or pass `--program <FILE.wasm>`.",
      fallback, e
    ).into()),
  }
}
