
use super::*;

use wasmtime::*;


pub async fn run_local(file_path: &std::path::PathBuf, args: &args::Args) -> DynResult<()> {

  let local_config = config::Config::read_from_file(&args.config).await.map_err(map_loc_err!())?;

  let wasm_bytes = tokio::fs::read(file_path).await.map_err(map_loc_err!())?;

  if crate::v_is_info() {
      tracing::info!("Running {:?} ({})", file_path, fs_utils::format_size_bytes(wasm_bytes.len()) );
  }

  let source = config::IdentityData::generate_from_config(&local_config).await.map_err(map_loc_err!())?;

  let pd = executor::ProgramDataBuilder::new()
    .set_human_name(
      file_path.file_name().map(|fn_osstr| fn_osstr.to_string_lossy().to_string() ).unwrap_or_else(|| "UNSET_NAME".to_string() )
    )
    .set_wasm_program_bytes(&wasm_bytes)
    .set_source(&source)
    .build().map_err(map_loc_err!())?;

  let executor = executor::Executor::new(&local_config).await;

  match executor.exec(&pd).await {
    Ok(res) => {
      tracing::info!("res = {:?}", res);
    }
    Err(e) => {
      tracing::info!("e = {:?}", e);
    }
  }

  Ok(())
}

pub async fn old_run_local(file_path: &std::path::PathBuf) -> DynResult<()> {
  let wasm_bytes = tokio::fs::read(file_path).await.map_err(map_loc_err!())?;

  if crate::v_is_info() {
      tracing::info!("Running {:?} ({}mb)", file_path, wasm_bytes.len() / 1_000_000);
  }

  // Configure the Wasmtime engine with fuel consumption enabled
  let mut config = Config::new();
  config.consume_fuel(true); // Enable fuel tracking for instruction counting
  config.async_support(true); // Affects APIs available

  let engine = Engine::new(&config).map_err(map_loc_err!())?;

  // // Build a MINIMAL WASI context:
  // //   - no filesystem
  // //   - no random
  // //   - no clocks
  // //   - only stdout & stderr
  let wasi_ctx = wasmtime_wasi::WasiCtxBuilder::new()
      .inherit_stdout()   // allow fd_write to stdout
      .inherit_stderr()   // allow fd_write to stderr
      // NOTE: do NOT call inherit_stdin()
      // NOTE: do NOT call preopen_dir()
      // NOTE: do NOT call inherit_args() unless you want argv
      // NOTE: do NOT call inherit_env() unless you want env vars
      //.build();
      .build_p1();

  // Create a store with our custom data
  let mut store = Store::new(&engine, StoreData::new(50, wasi_ctx));

  // Set initial fuel (roughly corresponds to instruction count)
  store.set_fuel(128_000).map_err(map_loc_err!())?;

  // Create a linker to bind our custom module
  let mut linker = Linker::new(&engine);

  wasmtime_wasi::p1::add_to_linker_async::<StoreData>(&mut linker, |linker_store_data| {
      &mut linker_store_data.wasi_p1_ctx
  }).map_err(map_loc_err!())?;

  // Bind a custom "env" module with a "log" function
  linker.func_wrap(
      "env",
      "log",
      |_caller: Caller<'_, StoreData>, ptr: i32, len: i32| -> Result<()> {
          tracing::info!("[WASM] Log called with ptr={}, len={}", ptr, len);
          Ok(())
      },
  ).map_err(map_loc_err!())?;

  // Add another custom function that returns a value
  linker.func_wrap(
      "env",
      "get_magic_number",
      |_caller: Caller<'_, StoreData>| -> Result<i32> {
          tracing::info!("[HOST] get_magic_number called, returning 42");
          Ok(42)
      },
  ).map_err(map_loc_err!())?;

  let module = Module::new(&engine, &wasm_bytes).map_err(map_loc_err!())?;

  if crate::v_is_debug() {
      tracing::info!("");
      for (i, import) in module.imports().enumerate() {
        tracing::info!("Import {} = {:?}", i, import);
      }
      tracing::info!("");
      for (i, export) in module.exports().enumerate() {
        tracing::info!("Export {} = {:?}", i, export);
      }
      tracing::info!("");
  }

  //debug_all_imports(&mut linker, &mut store, &module).map_err(map_loc_err!())?;

  let instance = linker.instantiate_async(&mut store, &module).await.map_err(map_loc_err!())?;

  // Get the exported function we want to call
  //let main_func = instance.get_typed_func::<(), i32>(&mut store, "_start").map_err(map_loc_err!())?;
  let main_func = instance.get_typed_func::<(), ()>(&mut store, "_start").map_err(map_loc_err!())?;

  println!("--- Executing WASM function ---");
  let initial_fuel = store.get_fuel().map_err(map_loc_err!())?;
  println!("Initial fuel: {}", initial_fuel);

  // Execute the function and track fuel consumption
  let result = main_func.call_async(&mut store, ()).await.map_err(map_loc_err!())?;

  let remaining_fuel = store.get_fuel().map_err(map_loc_err!())?;
  let consumed_fuel = initial_fuel - remaining_fuel;

  println!("\n--- Execution Complete ---");
  println!("Result: {:?}", result);
  println!("Fuel (instructions) consumed: {}", consumed_fuel);
  println!("Remaining fuel: {}", remaining_fuel);


  Ok(())
}

/// Store data that tracks instruction count
struct StoreData {
    instruction_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    max_instructions: u64,
    wasi_p1_ctx: wasmtime_wasi::p1::WasiP1Ctx,
}

unsafe impl Send for StoreData {}

impl StoreData {
    fn new(max_instructions: u64, wasi_p1_ctx: wasmtime_wasi::p1::WasiP1Ctx) -> Self {
        Self {
            instruction_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            max_instructions: max_instructions,
            wasi_p1_ctx: wasi_p1_ctx,
        }
    }

    fn get_count(&self) -> u64 {
        self.instruction_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

