//! The plugin system's host side: the policy a deployer writes, the registry
//! of loaded plugins and the refusals that keep the rest serving
//! ([ADR 0080](../../../../docs/adr/0080-one-plugin-system-three-tiers.md)).

mod assemble;
mod events;
mod policy;
mod registry;
mod routes;
#[cfg(test)]
mod tests;

pub use assemble::{InstallError, STATUS_PATH, install};
pub use events::{Counts, Dispatcher, QUEUE};
pub use policy::{Entry, Policy, PolicyError, PolicyTier};
pub use registry::{Plugin, Registry, State, Taken};
pub use routes::mount;
