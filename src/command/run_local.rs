
use super::*;


pub async fn run_local(file_path: &std::path::PathBuf) -> DynResult<()> {
  let wasm_bytes = tokio::fs::read(file_path).await?;

  tracing::info!("Running {:?} ({}mb)", file_path, wasm_bytes.len() / 1_000_000);

  // Configure the Wasmtime engine with fuel consumption enabled
  let mut config = Config::new();
  config.consume_fuel(true); // Enable fuel tracking for instruction counting

  let engine = Engine::new(&config)?;

  // Create a store with our custom data
  let mut store = Store::new(&engine, StoreData::new(50));

  // Set initial fuel (roughly corresponds to instruction count)
  store.set_fuel(10000)?;

  // Create a linker to bind our custom module
  let mut linker = Linker::new(&engine);

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

  for (i, import) in module.imports().enumerate() {
    tracing::info!("Import {} = {:?}", i, import);
  }

  for (i, export) in module.exports().enumerate() {
    tracing::info!("Export {} = {:?}", i, export);
  }

  let instance = linker.instantiate(&mut store, &module)?;

  // Get the exported function we want to call
  let main_func = instance.get_typed_func::<(), i32>(&mut store, "_start")?;

  println!("--- Executing WASM function ---");
  let initial_fuel = store.get_fuel()?;
  println!("Initial fuel: {}", initial_fuel);

  // Execute the function and track fuel consumption
  let result = main_func.call(&mut store, ())?;

  let remaining_fuel = store.get_fuel()?;
  let consumed_fuel = initial_fuel - remaining_fuel;

  println!("\n--- Execution Complete ---");
  println!("Result: {}", result);
  println!("Fuel consumed (≈instructions): {}", consumed_fuel);
  println!("Remaining fuel: {}", remaining_fuel);


  Ok(())
}



use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use wasmtime::*;

/// Store data that tracks instruction count
struct StoreData {
    instruction_count: Arc<AtomicU64>,
    max_instructions: u64,
}

impl StoreData {
    fn new(max_instructions: u64) -> Self {
        Self {
            instruction_count: Arc::new(AtomicU64::new(0)),
            max_instructions,
        }
    }

    fn get_count(&self) -> u64 {
        self.instruction_count.load(Ordering::SeqCst)
    }
}

