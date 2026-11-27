//! `model/linear`: a weight for each feature, by name.

use super::ModelError;
use super::json::{at, number, record};
use crate::dsl::tokens::Kind;

/// The dot product of the weights with the feature vector.
#[derive(Debug)]
pub struct Linear {
    /// The weight of each feature of the set, in its order; 0 when absent.
    weights: Vec<f32>,
}

impl Linear {
    pub(super) fn parse(definition: &str, features: &[String]) -> Result<Self, ModelError> {
        let rec = record(definition)?;
        let mut p = rec.parser();
        if p.next_token() != Some(Kind::StartObject) {
            return Err(at(
                &p,
                "a linear model is an object of weights by feature name",
            ));
        }
        // ALLOC: one weight per feature of the set, at most MAX_FEATURES.
        let mut weights = vec![0.0f32; features.len()];
        loop {
            match p.next_token() {
                Some(Kind::EndObject) => return Ok(Self { weights }),
                Some(Kind::FieldName) => {
                    // ALLOC: one entry per feature or name of the definition, which is at most MAX_DEFINITION_BYTES.
                    let name = p.text().to_owned();
                    let Some(slot) = features.iter().position(|f| *f == name) else {
                        // ALLOC: one entry per feature or name of the definition, which is at most MAX_DEFINITION_BYTES.
                        return Err(ModelError::new(format!(
                            "the linear model names the feature [{name}], which is not in its feature set"
                        )));
                    };
                    p.next_token();
                    let weight = number(&p)?;
                    if let Some(w) = weights.get_mut(slot) {
                        *w = weight;
                    }
                }
                _ => return Err(at(&p, "expected a feature name")),
            }
        }
    }

    pub(super) fn score(&self, vector: &[f32]) -> f32 {
        self.weights
            .iter()
            .zip(vector)
            .map(|(w, x)| if x.is_nan() { 0.0 } else { w * x })
            .sum()
    }
}
