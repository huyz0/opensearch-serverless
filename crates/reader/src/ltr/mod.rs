//! Learning to rank: the evaluators of a stored model.
//!
//! A model's `definition` is parsed once into an immutable [`Evaluator`]
//! that scores a feature vector. The vector holds one `f32` per feature of
//! the model's feature set, in the set's order; `NaN` marks a feature whose
//! query did not match.

pub(crate) mod json;
mod linear;
pub mod models;
mod ranklib;
mod xgboost;

#[cfg(test)]
mod tests;

use std::fmt;

/// The largest definition a model may carry.
pub const MAX_DEFINITION_BYTES: usize = 10 << 20;
/// The most trees a model may carry.
pub const MAX_TREES: usize = 4_096;
/// The deepest a tree may nest.
pub const MAX_DEPTH: usize = 64;
/// The most features a feature set may have.
pub const MAX_FEATURES: usize = 10_000;

/// Why a model was refused; the message is the `reason` of a 400.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelError(String);

impl ModelError {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    /// The reason, as the REST error shows it.
    pub fn reason(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelError {}

/// A parsed model.
#[derive(Debug)]
pub enum Evaluator {
    /// `model/linear`.
    Linear(linear::Linear),
    /// `model/xgboost+json`.
    Xgboost(xgboost::Xgboost),
    /// `model/ranklib`.
    Ranklib(ranklib::Ranklib),
}

impl Evaluator {
    /// Parses a model of `kind` (`model/linear`, `model/xgboost+json`,
    /// `model/ranklib`) whose features are `features`, in order.
    pub fn parse(kind: &str, definition: &str, features: &[String]) -> Result<Self, ModelError> {
        if definition.len() > MAX_DEFINITION_BYTES {
            return Err(ModelError::new(format!(
                "the model definition is {} bytes, over the limit of {MAX_DEFINITION_BYTES}",
                definition.len()
            )));
        }
        if features.len() > MAX_FEATURES {
            return Err(ModelError::new(format!(
                "the feature set has {} features, over the limit of {MAX_FEATURES}",
                features.len()
            )));
        }
        match kind {
            "model/linear" => linear::Linear::parse(definition, features).map(Self::Linear),
            "model/xgboost+json" => {
                xgboost::Xgboost::parse(definition, features.len()).map(Self::Xgboost)
            }
            "model/ranklib" => {
                ranklib::Ranklib::parse(definition, features.len()).map(Self::Ranklib)
            }
            other => Err(ModelError::new(format!("unknown model type [{other}]"))),
        }
    }

    /// The model's score of `vector`. A missing entry or a `NaN` is a
    /// feature that did not match.
    pub fn score(&self, vector: &[f32]) -> f32 {
        match self {
            Self::Linear(m) => m.score(vector),
            Self::Xgboost(m) => m.score(vector),
            Self::Ranklib(m) => m.score(vector),
        }
    }
}
