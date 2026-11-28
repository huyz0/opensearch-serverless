//! The anomaly model of the native anomaly detection plugin.
//!
//! A Random Cut Forest (Guha et al., 2016) over shingled feature vectors, a
//! thresholder that turns its score into a grade and a confidence, imputation
//! of a missing window, and a checksummed checkpoint a node restores after an
//! ownership change. Pure compute: no I/O, no clock, no threads. Memory is
//! fixed at construction ([`Params::reserved_bytes`]) and feeding a window
//! allocates nothing.
//!
//! ```
//! use oss_anomaly::{Model, Params, Workspace};
//!
//! let mut params = Params::new(1, 7);
//! params.shingle_size = 2;
//! let mut ws = Workspace::new(&params);
//! let mut model = Model::new(params)?;
//! for slot in 0..300u64 {
//!     let value = [(slot % 10) as f32];
//!     model.feed(&mut ws, slot, Some(&value))?;
//! }
//! let spike = model.feed(&mut ws, 300, Some(&[500.0]))?.expect("ready");
//! assert!(spike.grade > 0.0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod checkpoint;
mod forest;
mod model;
mod params;
mod rng;
mod shingle;
mod threshold;
mod tree;
mod workspace;

#[cfg(test)]
mod tests;

pub use checkpoint::RestoreError;
pub use model::{FeedError, Model, Scored};
pub use params::{
    Imputation, MAX_DIMS, MAX_FEATURES, MAX_SAMPLE, MAX_SHINGLE, MAX_TREES, MIN_SAMPLE, ParamError,
    Params,
};
pub use workspace::Workspace;
