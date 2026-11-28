//! The plugin system's host side: the policy a deployer writes, the registry
//! of loaded plugins and the refusals that keep the rest serving
//! ([ADR 0080](../../../../docs/adr/0080-one-plugin-system-three-tiers.md)).

mod assemble;
mod events;
mod policy;
mod registry;
mod routes;
mod state;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod tests;
#[cfg(feature = "extensions")]
mod wasm;
#[cfg(feature = "extensions")]
mod wasm_load;

pub use assemble::{InstallError, STATUS_PATH, install};
#[cfg(feature = "writer")]
pub(crate) use assemble::{configured_modules, install_configured};
pub use events::{Counts, Dispatcher, QUEUE};
pub use policy::{Entry, Policy, PolicyError, PolicyTier};
pub use registry::{Plugin, Registry, State, Taken};
pub use routes::mount;
pub use state::StateBackend;
#[cfg(feature = "extensions")]
pub use wasm::WasmPlugin;
#[cfg(feature = "extensions")]
pub use wasm_load::load_modules;
