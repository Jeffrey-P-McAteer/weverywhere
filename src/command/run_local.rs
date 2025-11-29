
use super::*;

pub async fn run_local(file_path: &std::path::PathBuf) -> DynResult<()> {
  let wasm_bytes = tokio::fs::read(file_path).await?;

  if crate::v_is_info() {
      tracing::info!("Running {:?} ({}mb)", file_path, wasm_bytes.len() / 1_000_000);
  }

  // Configure the Wasmtime engine with fuel consumption enabled
  let mut config = Config::new();
  config.consume_fuel(true); // Enable fuel tracking for instruction counting
  config.async_support(true); // Affects APIs available

  let engine = Engine::new(&config)?;

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
  store.set_fuel(128_000)?;

  // Create a linker to bind our custom module
  let mut linker = Linker::new(&engine);

  wasmtime_wasi::p1::add_to_linker_async::<StoreData>(&mut linker, |linker_store_data| {
      &mut linker_store_data.wasi_p1_ctx
  })?;

  // Bind a custom "env" module with a "log" function
  linker.func_wrap(
      "env",
      "log",
      |_caller: Caller<'_, StoreData>, ptr: i32, len: i32| -> Result<()> {
          tracing::info!("[WASM] Log called with ptr={}, len={}", ptr, len);
          Ok(())
      },
  )?;

  // Add another custom function that returns a value
  linker.func_wrap(
      "env",
      "get_magic_number",
      |_caller: Caller<'_, StoreData>| -> Result<i32> {
          tracing::info!("[HOST] get_magic_number called, returning 42");
          Ok(42)
      },
  )?;

  let module = Module::new(&engine, &wasm_bytes)?;

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

  //debug_all_imports(&mut linker, &mut store, &module)?;

  let instance = linker.instantiate_async(&mut store, &module).await?;

  // Get the exported function we want to call
  //let main_func = instance.get_typed_func::<(), i32>(&mut store, "_start")?;
  let main_func = instance.get_typed_func::<(), ()>(&mut store, "_start")?;

  println!("--- Executing WASM function ---");
  let initial_fuel = store.get_fuel()?;
  println!("Initial fuel: {}", initial_fuel);

  // Execute the function and track fuel consumption
  let result = main_func.call_async(&mut store, ()).await?;

  let remaining_fuel = store.get_fuel()?;
  let consumed_fuel = initial_fuel - remaining_fuel;

  println!("\n--- Execution Complete ---");
  println!("Result: {:?}", result);
  println!("Fuel (instructions) consumed: {}", consumed_fuel);
  println!("Remaining fuel: {}", remaining_fuel);


  Ok(())
}

// // AI-generated experiment we are replacing with a proper WASI runtime that is locked-down or has overridden functions.
// fn debug_all_imports<T>(
//     linker: &mut Linker<T>,
//     store: &mut Store<T>,
//     module: &Module,
// ) -> DynResult<()> {

//     for import in module.imports() {
//         let ExternType::Func(func_ty) = import.ty() else {
//             continue;
//         };

//         let module_name = import.module().to_string();
//         let func_name = import.name().to_string();

//         let mod_name = module_name.clone();
//         let fn_name = func_name.clone();
//         let result_types: Vec<_> = func_ty.results().collect();

//         // 🔥 IMPORTANT: func is created FROM THE STORE, not the engine
//         let host_func = Func::new(
//             store.as_context_mut(),
//             func_ty.clone(),
//             move |_caller: Caller<'_, T>, args: &[Val], results: &mut [Val]| {
//                 println!("called import {}::{}", mod_name, fn_name);
//                 println!("  args: {:?}", args);

//                 for (i, ty) in result_types.iter().enumerate() {
//                     results[i] = match ty {
//                         ValType::I32 => Val::I32(0),
//                         ValType::I64 => Val::I64(0),
//                         ValType::F32 => Val::F32(0f32.to_bits()),
//                         ValType::F64 => Val::F64(0f64.to_bits()),
//                         ValType::V128 => Val::V128(0.into()),
//                         ValType::Ref(_ref) => Val::AnyRef(None),
//                         //_ => unimplemented!("result {:?}", ty),
//                     };
//                 }

//                 Ok(())
//             },
//         );

//         // 🔥 define also requires the store first
//         linker.define(
//             store.as_context_mut(),
//             &module_name,
//             &func_name,
//             host_func,
//         )?;
//     }

//     Ok(())
// }


use wasmtime::*;

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

