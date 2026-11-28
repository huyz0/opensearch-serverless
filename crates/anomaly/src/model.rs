//! A model: a shingle, a forest and a thresholder behind one `feed`.

// ARITH: counters are `saturating_add`ed; the only other arithmetic is on
// slots (`u64`, compared before it is subtracted) and bounded sizes.
#![allow(clippy::arithmetic_side_effects)]

use std::fmt;

use crate::forest::Forest;
use crate::params::{MAX_VALUE, ParamError, Params};
use crate::shingle::Shingle;
use crate::threshold::Thresholder;
use crate::workspace::Workspace;

/// What a window scored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scored {
    /// The forest's raw anomaly score.
    pub score: f64,
    /// The thresholder's grade, in [0, 1]; above zero is an anomaly.
    pub grade: f64,
    /// How much history the score rests on, in [0, 1].
    pub confidence: f64,
}

/// Why a window was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedError {
    /// The window does not have one value per feature.
    Width,
    /// A value is NaN or infinite.
    NotFinite,
    /// The slot is not after the last one fed.
    Stale,
    /// A value is larger in magnitude than `MAX_VALUE`.
    OutOfRange,
    /// The workspace is smaller than the model needs.
    Workspace,
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Width => "a window needs one value per feature",
            Self::NotFinite => "a window value is not a finite number",
            Self::Stale => "the slot is not after the last slot fed",
            Self::OutOfRange => "a window value is too large in magnitude",
            Self::Workspace => "the workspace is smaller than the model needs",
        })
    }
}

impl std::error::Error for FeedError {}

/// One detector's model, or one entity's.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub(crate) params: Params,
    pub(crate) shingle: Shingle,
    pub(crate) forest: Forest,
    pub(crate) thresh: Thresholder,
    pub(crate) last_fed_slot: Option<u64>,
    pub(crate) points_seen: u64,
    window: Scratch,
    point: Scratch,
}

/// Scratch space that is never part of a model's identity: two models are equal
/// whatever their scratch holds.
#[derive(Clone, Debug)]
struct Scratch(Vec<f32>);

impl PartialEq for Scratch {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Model {
    /// Builds a model, allocating everything it will ever hold
    /// ([`Params::reserved_bytes`]); reserve those bytes first.
    pub fn new(params: Params) -> Result<Self, ParamError> {
        params.validate()?;
        Ok(Self::assemble(params))
    }

    pub(crate) fn assemble(params: Params) -> Self {
        Self {
            shingle: Shingle::new(&params),
            forest: Forest::new(&params),
            thresh: Thresholder::new(params.sample_size),
            last_fed_slot: None,
            points_seen: 0,
            window: Scratch(vec![0.0; params.features]),
            point: Scratch(vec![0.0; params.dims()]),
            params,
        }
    }

    /// The model's parameters.
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// The slot of the last window fed, which a checkpoint carries so a new
    /// owner knows how many windows to impute.
    pub fn last_fed_slot(&self) -> Option<u64> {
        self.last_fed_slot
    }

    /// Whether the forest holds enough points to score.
    pub fn ready(&self) -> bool {
        self.forest.len() >= self.params.min_train_samples
    }

    /// Feeds the window of `slot` (`None` when the source had no data) and
    /// scores it. Slots between the last one fed and this one are imputed up
    /// to `max_gap`; a longer gap restarts the shingle. Returns `None` while
    /// the shingle or the forest is still filling, and for a window with no data.
    pub fn feed(
        &mut self,
        ws: &mut Workspace,
        slot: u64,
        values: Option<&[f32]>,
    ) -> Result<Option<Scored>, FeedError> {
        if !ws.fits(&self.params) {
            return Err(FeedError::Workspace);
        }
        if let Some(v) = values {
            if v.len() != self.params.features {
                return Err(FeedError::Width);
            }
            if v.iter().any(|x| !x.is_finite()) {
                return Err(FeedError::NotFinite);
            }
            if v.iter().any(|x| x.abs() > MAX_VALUE) {
                return Err(FeedError::OutOfRange);
            }
        }
        if let Some(last) = self.last_fed_slot {
            if slot <= last {
                return Err(FeedError::Stale);
            }
            let gap = slot - last - 1;
            if gap > self.params.max_gap {
                self.shingle.clear();
            } else {
                for _ in 0..gap {
                    self.step(ws, None);
                }
            }
        }
        self.last_fed_slot = Some(slot);
        Ok(self.step(ws, values))
    }

    fn step(&mut self, ws: &mut Workspace, values: Option<&[f32]>) -> Option<Scored> {
        match values {
            Some(v) => self.window.0.copy_from_slice(v),
            None => self
                .shingle
                .imputed(&self.params.imputation, &mut self.window.0),
        }
        self.shingle.push(&self.window.0);
        if !self.shingle.full() {
            return None;
        }
        self.shingle.point(&mut self.point.0);
        let scored = if values.is_some() && self.ready() {
            let score = self.forest.score(ws, &self.point.0);
            let grade = self.thresh.grade(score);
            if score.is_finite() {
                self.thresh.update(score);
            }
            let confidence = (self.forest.len() as f64 / self.params.sample_size as f64).min(1.0);
            Some(Scored {
                score,
                grade,
                confidence,
            })
        } else {
            None
        };
        self.forest.update(ws, &self.point.0);
        self.points_seen = self.points_seen.saturating_add(1);
        scored
    }
}
