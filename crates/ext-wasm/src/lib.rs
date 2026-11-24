//! The WASM host for extension modules
//! ([ADR 0052](../../../docs/adr/0052-extension-modules-on-a-wasm-abi.md)).
//!
//! A module is a core WebAssembly module with no WASI that imports only
//! `oss.log` and `oss.cancelled`. [`Host::load`] refuses a module over fixed
//! limits before any compile, compiles it, and reads its descriptor;
//! [`Module::call`] runs one call on the caller's thread in a fresh instance
//! bounded by fuel, linear memory, a deadline and a stack, behind a
//! concurrency gate, with every byte reserved first. No point is wired to a
//! request path here.

mod analyzer;
mod config;
mod gate;
mod host;
mod module;
mod reserve;
mod validate;

pub use analyzer::PinRefusal;
pub use config::HostConfig;
pub use host::{Host, LoadError};
pub use module::Module;
pub use reserve::{Reservation, Reserver, Unbounded};
