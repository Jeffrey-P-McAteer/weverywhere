
//! Access to WASI example programs that were compiled and embedded into the binary at build time.
//!
//! `build.rs` compiles each program listed in `example-programs/embedded.list` (with zig) and
//! generates the `EMBEDDED_PROGRAMS` table `include!`-ed below. Commands that ship a bundled program
//! (e.g. `netmap`) run these in-memory bytes when no `--program` override is given, so a copied
//! binary is self-contained and needs no external `.wasm` files. Users can dump every embedded
//! program back to disk with `weverywhere extract-programs <DIR>`.
//!
//! If zig was unavailable at build time the table is empty; callers then fall back to `--program`
//! or the on-disk compiled example.

// Generated: `pub static EMBEDDED_PROGRAMS: &[(&str, &[u8])] = &[ ... ];`
include!(concat!(env!("OUT_DIR"), "/embedded_programs.rs"));

/// Bytes of the embedded program with this stem (e.g. "network-map"), if it was compiled in.
pub fn get(name: &str) -> Option<&'static [u8]> {
  EMBEDDED_PROGRAMS.iter().find(|(n, _)| *n == name).map(|(_, bytes)| *bytes)
}

/// Every embedded program as `(name, bytes)`. Empty if nothing was embedded at build time.
pub fn all() -> &'static [(&'static str, &'static [u8])] {
  EMBEDDED_PROGRAMS
}
