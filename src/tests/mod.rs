//! All unit/integration tests live here, one file per component, rather than inline `#[cfg(test)]`
//! blocks next to the code. Keeping tests in a separate module tree means they can only reach items
//! that are actually `pub`, so the test suite exercises each component through its public surface.

mod config;
mod crypto_utils;
mod discovery;
mod executor;
mod messages;
mod tty;
