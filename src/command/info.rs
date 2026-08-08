
use super::*;

/// Print metadata about a WASI/WASM module: its size, whether it looks like a Program
/// (has a `_start` entry point) or a Library (see the vocabulary in readme.md), the WASI
/// flavor it imports, and the full list of imported and exported items with their signatures.
pub async fn info(file_path: &std::path::PathBuf) -> DynResult<()> {
  let wasm_bytes = tokio::fs::read(file_path).await.map_err(map_loc_err!())?;

  // A default engine is all we need to validate + introspect a core wasm module.
  let engine = wasmtime::Engine::default();
  let module = wasmtime::Module::new(&engine, &wasm_bytes).map_err(map_loc_err!())?;

  let has_start = module.exports().any(|e| e.name() == "_start");
  let wasi_flavor = detect_wasi_flavor(&module);

  println!("File:    {}", file_path.display());
  println!("Size:    {}", fs_utils::format_size_bytes(wasm_bytes.len()));
  println!("Kind:    {}", if has_start { "Program (has _start entry point)" } else { "Library" });
  println!("WASI:    {}", wasi_flavor);

  let imports: Vec<_> = module.imports().collect();
  println!("\nImports ({}):", imports.len());
  if imports.is_empty() {
    println!("  (none)");
  }
  for imp in &imports {
    println!("  {}::{}  {}", imp.module(), imp.name(), describe_extern(&imp.ty()));
  }

  let exports: Vec<_> = module.exports().collect();
  println!("\nExports ({}):", exports.len());
  if exports.is_empty() {
    println!("  (none)");
  }
  for exp in &exports {
    println!("  {}  {}", exp.name(), describe_extern(&exp.ty()));
  }

  Ok(())
}

/// Guess which WASI ABI a module targets from the modules it imports from.
fn detect_wasi_flavor(module: &wasmtime::Module) -> &'static str {
  let mut saw_wasi = false;
  for imp in module.imports() {
    match imp.module() {
      "wasi_snapshot_preview1" => return "preview1 (wasi_snapshot_preview1)",
      "wasi_unstable"          => return "preview0 (wasi_unstable)",
      m if m.starts_with("wasi:") => saw_wasi = true,
      _ => {}
    }
  }
  if saw_wasi { "preview2 (component-model style imports)" } else { "none detected (plain wasm)" }
}

/// A short human description of an imported/exported item's type. Function signatures are
/// rendered explicitly; other kinds fall back to a compact debug form.
fn describe_extern(ty: &wasmtime::ExternType) -> String {
  match ty {
    wasmtime::ExternType::Func(ft) => {
      let params: Vec<String> = ft.params().map(|p| valtype_str(&p)).collect();
      let results: Vec<String> = ft.results().map(|r| valtype_str(&r)).collect();
      format!("func ({}) -> ({})", params.join(", "), results.join(", "))
    }
    wasmtime::ExternType::Memory(mt) => {
      match mt.maximum() {
        Some(max) => format!("memory {}..{} pages", mt.minimum(), max),
        None => format!("memory {}.. pages", mt.minimum()),
      }
    }
    wasmtime::ExternType::Global(gt) => {
      let m = if gt.mutability() == wasmtime::Mutability::Var { "mut " } else { "" };
      format!("global {}{}", m, valtype_str(gt.content()))
    }
    other => format!("{:?}", other),
  }
}

fn valtype_str(v: &wasmtime::ValType) -> String {
  match v {
    wasmtime::ValType::I32 => "i32".to_string(),
    wasmtime::ValType::I64 => "i64".to_string(),
    wasmtime::ValType::F32 => "f32".to_string(),
    wasmtime::ValType::F64 => "f64".to_string(),
    wasmtime::ValType::V128 => "v128".to_string(),
    other => format!("{:?}", other),
  }
}
