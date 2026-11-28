//! Cross-cluster replication, slice 1: remotes and a read-only reader of a
//! leader's store (`docs/specs/replication/SPEC.md`).

pub mod remote;
pub mod settings;

pub use remote::{LeaderIndex, RemoteError, RemoteStore, ShardManifest};
pub use settings::{Location, Remote, SettingsError, remotes};
