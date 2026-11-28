//! The parameters of a model and the memory they imply.

use std::fmt;
use std::mem::size_of;

use crate::tree::Node;

/// The most trees a forest may have.
pub const MAX_TREES: usize = 100;
/// The fewest and most points one tree samples.
pub const MIN_SAMPLE: usize = 8;
/// The most points one tree samples.
pub const MAX_SAMPLE: usize = 2_048;
/// The most steps a shingle may span.
pub const MAX_SHINGLE: usize = 60;
/// The most features one window may carry.
pub const MAX_FEATURES: usize = 64;
/// The most values in one point: features times shingle steps.
pub const MAX_DIMS: usize = 512;
/// The most missing windows a gap may be imputed over.
pub const MAX_GAP: u64 = 1_024;
/// The largest magnitude a window value may have; a difference of two stays far from `f32::MAX`.
pub const MAX_VALUE: f32 = 1.0e18;
/// The ring of recent points holds this many times `sample_size`.
pub(crate) const STORE_FACTOR: usize = 4;
/// Memory every model holds besides its arrays: the shingle, thresholder and bookkeeping.
const FIXED_BYTES: usize = 16 << 10;

/// How a window with no data is filled in.
#[derive(Clone, Debug, PartialEq)]
pub enum Imputation {
    /// Every feature is zero.
    Zero,
    /// Each feature takes its own value from this list (one per feature).
    Fixed(Vec<f32>),
    /// Each feature repeats the last window's value (zero before there is one).
    Previous,
}

/// A parameter that is out of range; the text names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamError(pub String);

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParamError {}

/// The parameters of one model.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Trees in the forest (1 to 100, default 30).
    pub trees: usize,
    /// Points each tree samples (8 to 2,048, default 256).
    pub sample_size: usize,
    /// Windows in a point (1 to 60, default 8).
    pub shingle_size: usize,
    /// Values in one window (1 to 64).
    pub features: usize,
    /// The decay of a point's sampling weight per step; `0` samples uniformly.
    pub time_decay: f64,
    /// The seed of every random choice.
    pub seed: u64,
    /// Points the forest must hold before it scores (default 128).
    pub min_train_samples: usize,
    /// Missing windows imputed before the shingle restarts (default 2).
    pub max_gap: u64,
    /// How a missing window is filled in.
    pub imputation: Imputation,
}

impl Params {
    /// The plugin's defaults for a detector with `features` features.
    pub fn new(features: usize, seed: u64) -> Self {
        Self {
            trees: 30,
            sample_size: 256,
            shingle_size: 8,
            features,
            time_decay: 1.0 / 512.0,
            seed,
            min_train_samples: 128,
            max_gap: 2,
            imputation: Imputation::Zero,
        }
    }

    /// Checks every bound; the error names the parameter.
    pub fn validate(&self) -> Result<(), ParamError> {
        let bad = |what: &str| Err(ParamError(format!("{what} is out of range")));
        if !(1..=MAX_TREES).contains(&self.trees) {
            return bad("number_of_trees");
        }
        if !(MIN_SAMPLE..=MAX_SAMPLE).contains(&self.sample_size) {
            return bad("sample_size");
        }
        if !(1..=MAX_SHINGLE).contains(&self.shingle_size) {
            return bad("shingle_size");
        }
        if !(1..=MAX_FEATURES).contains(&self.features) {
            return bad("features");
        }
        if self.dims() > MAX_DIMS {
            return bad("features times shingle_size");
        }
        if !(self.time_decay >= 0.0 && self.time_decay < 1.0) {
            return bad("time_decay");
        }
        if self.max_gap > MAX_GAP {
            return bad("max_gap");
        }
        if self.min_train_samples < 2 || self.min_train_samples > self.sample_size {
            return bad("min_train_samples");
        }
        if let Imputation::Fixed(v) = &self.imputation
            && (v.len() != self.features || v.iter().any(|x| !x.is_finite() || x.abs() > MAX_VALUE))
        {
            return bad("fixed imputation values");
        }
        Ok(())
    }

    /// Values in one point.
    pub fn dims(&self) -> usize {
        self.features.saturating_mul(self.shingle_size)
    }

    /// Points in the ring of recent points.
    pub(crate) fn store_capacity(&self) -> usize {
        self.sample_size.saturating_mul(STORE_FACTOR)
    }

    /// Nodes in one tree's arena.
    pub(crate) fn node_capacity(&self) -> usize {
        self.sample_size.saturating_mul(2).saturating_sub(1)
    }

    /// The bytes a model holds: the capacity of every array it owns. Reserve
    /// this before building one.
    pub fn reserved_bytes(&self) -> usize {
        let per_tree = self
            .node_capacity()
            .saturating_mul(size_of::<Node>())
            .saturating_add(self.node_capacity().saturating_mul(size_of::<u32>()))
            .saturating_add(
                self.sample_size
                    .saturating_mul(size_of::<crate::forest::Held>()),
            )
            .saturating_add(self.store_capacity().saturating_mul(size_of::<u32>()));
        per_tree
            .saturating_mul(self.trees)
            .saturating_add(
                self.store_capacity()
                    .saturating_mul(self.dims())
                    .saturating_mul(size_of::<f32>()),
            )
            .saturating_add(
                self.dims()
                    .saturating_mul(3)
                    .saturating_mul(size_of::<f32>()),
            )
            .saturating_add(FIXED_BYTES)
    }

    /// The bytes one worker's [`crate::Workspace`] holds; shared by every model it runs.
    pub fn workspace_bytes(&self) -> usize {
        let nodes = self.node_capacity();
        nodes
            .saturating_mul(self.dims())
            .saturating_mul(2)
            .saturating_mul(size_of::<f32>())
            .saturating_add(nodes.saturating_mul(2).saturating_mul(size_of::<u32>()))
    }
}
