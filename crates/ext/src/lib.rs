//! The extension module contract that does not depend on how a module is
//! loaded ([ADR 0052](../../../docs/adr/0052-extension-modules-on-a-wasm-abi.md)).
//!
//! The version rule, the status codes, the ceilings, the strict descriptor
//! parser and the typed failure of a call. A host (the WASM one lives in its
//! own crate) judges a module by these alone, so a module is judged the same
//! way whatever loads it. Nothing here runs a module or allocates by a size a
//! module reports.

pub mod analyzer;
mod descriptor;
mod error;
mod json;
mod status;
mod version;

pub use descriptor::{Descriptor, DescriptorError, Limits, Point};
pub use error::{CallError, FailureKind, MAX_ERROR_DETAIL_BYTES, printable};
pub use status::{Status, status_of};
pub use version::{
    ABI_MAJOR, ABI_MINOR, ABI_VERSION, VersionError, abi_accepted, abi_accepted_against, pack_abi,
    unpack_abi,
};

/// The most bytes a descriptor may hold.
pub const MAX_DESCRIPTOR_BYTES: u32 = 16 * 1024;
/// The most bytes of opaque module configuration a host passes.
pub const MAX_CONFIG_BYTES: u32 = 64 * 1024;
/// The most bytes a module may be.
pub const MAX_MODULE_BYTES: u32 = 8 * 1024 * 1024;
/// The most bytes one I/O region may be; a point may only lower it.
pub const MAX_IO_BYTES: u32 = 16 * 1024 * 1024;
