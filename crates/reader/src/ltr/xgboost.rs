//! `model/xgboost+json`: a gbtree model as XGBoost saves it with
//! `save_model("model.json")`.

use super::json::{at, each_field, number, numbers, record};
use super::{MAX_DEPTH, MAX_TREES, ModelError};
use crate::dsl::tokens::{Kind, Parser};

/// What the margin becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Objective {
    /// `reg:squarederror`: the margin.
    Margin,
    /// `reg:logistic` and `binary:logistic`: the sigmoid of the margin.
    Logistic,
}

#[derive(Debug, Default)]
struct Tree {
    left: Vec<i32>,
    right: Vec<i32>,
    index: Vec<i32>,
    /// A split's threshold, a leaf's value.
    value: Vec<f32>,
    default_left: Vec<bool>,
}

/// A parsed gbtree.
#[derive(Debug)]
pub struct Xgboost {
    trees: Vec<Tree>,
    base_margin: f32,
    objective: Objective,
}

impl Xgboost {
    pub(super) fn parse(definition: &str, features: usize) -> Result<Self, ModelError> {
        let rec = record(definition)?;
        let mut p = rec.parser();
        p.next_token();
        let mut learner: Option<Self> = None;
        each_field(&mut p, |name, p| {
            if name == "learner" {
                learner = Some(learner_of(p, features)?);
            } else {
                p.skip_children();
            }
            Ok(())
        })?;
        learner.ok_or_else(|| ModelError::new("the XGBoost model has no [learner]"))
    }

    pub(super) fn score(&self, vector: &[f32]) -> f32 {
        let mut margin = self.base_margin;
        for tree in &self.trees {
            margin += tree.leaf(vector);
        }
        match self.objective {
            Objective::Margin => margin,
            Objective::Logistic => 1.0 / (1.0 + (-margin).exp()),
        }
    }
}

impl Tree {
    fn leaf(&self, vector: &[f32]) -> f32 {
        let mut node = 0usize;
        // The depth was checked at parse, so this ends.
        for _ in 0..=MAX_DEPTH {
            let left = self.left.get(node).copied().unwrap_or(-1);
            if left < 0 {
                return self.value.get(node).copied().unwrap_or(0.0);
            }
            let feature = usize::try_from(self.index.get(node).copied().unwrap_or(0)).unwrap_or(0);
            let x = vector.get(feature).copied().unwrap_or(f32::NAN);
            let goes_left = if x.is_nan() {
                self.default_left.get(node).copied().unwrap_or(true)
            } else {
                x < self.value.get(node).copied().unwrap_or(0.0)
            };
            let next = if goes_left {
                left
            } else {
                self.right.get(node).copied().unwrap_or(-1)
            };
            node = usize::try_from(next).unwrap_or(0);
        }
        0.0
    }

    /// Refuses a tree whose arrays disagree, whose split names a feature
    /// the set lacks, whose children leave it, or that is deeper than a
    /// model may be.
    fn check(&self, features: usize) -> Result<(), ModelError> {
        let n = self.left.len();
        if [
            self.right.len(),
            self.index.len(),
            self.value.len(),
            self.default_left.len(),
        ]
        .iter()
        .any(|&len| len != n)
            || n == 0
        {
            return Err(ModelError::new(
                "an XGBoost tree's node arrays are empty or differ in length",
            ));
        }
        // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
        let mut stack = vec![(0usize, 1usize)];
        let mut visited = 0usize;
        while let Some((node, depth)) = stack.pop() {
            visited = visited.saturating_add(1);
            if depth > MAX_DEPTH {
                // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                return Err(ModelError::new(format!(
                    "an XGBoost tree is deeper than {MAX_DEPTH}"
                )));
            }
            if visited > n {
                return Err(ModelError::new("an XGBoost tree's nodes are not a tree"));
            }
            let left = self.left[node];
            if left < 0 {
                continue;
            }
            let feature = usize::try_from(self.index[node]).unwrap_or(usize::MAX);
            if feature >= features {
                // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                return Err(ModelError::new(format!(
                    "an XGBoost split names the feature number {}, but the feature set has {features}",
                    self.index[node]
                )));
            }
            for child in [left, self.right[node]] {
                match usize::try_from(child) {
                    Ok(c) if c < n => stack.push((c, depth.saturating_add(1))),
                    _ => return Err(ModelError::new("an XGBoost child is outside its tree")),
                }
            }
        }
        Ok(())
    }
}

fn learner_of(p: &mut Parser<'_>, features: usize) -> Result<Xgboost, ModelError> {
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
    let mut trees = Vec::new();
    let mut base_score = 0.5f32;
    let mut objective = None;
    each_field(p, |name, p| {
        match name {
            "learner_model_param" => each_field(p, |name, p| {
                if name == "base_score" {
                    base_score = scalar(p)?;
                } else {
                    p.skip_children();
                }
                Ok(())
            })?,
            "objective" => each_field(p, |name, p| {
                if name == "name" {
                    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                    objective = Some(p.text().to_owned());
                } else {
                    p.skip_children();
                }
                Ok(())
            })?,
            "gradient_booster" => trees = booster(p)?,
            _ => p.skip_children(),
        }
        Ok(())
    })?;
    let objective = match objective.as_deref() {
        Some("reg:squarederror") => Objective::Margin,
        Some("reg:logistic" | "binary:logistic") => Objective::Logistic,
        Some(other) => {
            // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
            return Err(ModelError::new(format!(
                "the XGBoost objective [{other}] is not supported; use reg:squarederror, reg:logistic or binary:logistic"
            )));
        }
        None => return Err(ModelError::new("the XGBoost model has no objective")),
    };
    for tree in &trees {
        tree.check(features)?;
    }
    let base_margin = match objective {
        Objective::Margin => base_score,
        Objective::Logistic => {
            if !(base_score > 0.0 && base_score < 1.0) {
                return Err(ModelError::new(
                    "the XGBoost base_score of a logistic objective must lie between 0 and 1",
                ));
            }
            (base_score / (1.0 - base_score)).ln()
        }
    };
    Ok(Xgboost {
        trees,
        base_margin,
        objective,
    })
}

/// A number written as a number or as a string (XGBoost writes `base_score` as one).
fn scalar(p: &Parser<'_>) -> Result<f32, ModelError> {
    match p.current_token() {
        Some(Kind::Number) => number(p),
        Some(Kind::String) => p
            .text()
            .trim_matches(['[', ']'])
            .parse::<f32>()
            // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
            .map_err(|_| at(p, &format!("[{}] is not a number", p.text()))),
        _ => Err(at(p, "expected a number")),
    }
}

fn booster(p: &mut Parser<'_>) -> Result<Vec<Tree>, ModelError> {
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
    let mut trees = Vec::new();
    each_field(p, |name, p| {
        match name {
            "name" => {
                if p.text() != "gbtree" {
                    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                    return Err(ModelError::new(format!(
                        "the XGBoost booster [{}] is not supported; use gbtree",
                        p.text()
                    )));
                }
            }
            "model" => each_field(p, |name, p| {
                if name == "trees" {
                    trees = tree_list(p)?;
                } else {
                    p.skip_children();
                }
                Ok(())
            })?,
            _ => p.skip_children(),
        }
        Ok(())
    })?;
    Ok(trees)
}

fn tree_list(p: &mut Parser<'_>) -> Result<Vec<Tree>, ModelError> {
    if p.current_token() != Some(Kind::StartArray) {
        return Err(at(p, "expected the array of trees"));
    }
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
    let mut trees = Vec::new();
    while p.next_token() == Some(Kind::StartObject) {
        if trees.len() >= MAX_TREES {
            // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
            return Err(ModelError::new(format!(
                "the XGBoost model has more than {MAX_TREES} trees"
            )));
        }
        let mut tree = Tree::default();
        let most = super::MAX_DEFINITION_BYTES;
        each_field(p, |name, p| {
            match name {
                "left_children" => tree.left = ints(p, most)?,
                "right_children" => tree.right = ints(p, most)?,
                "split_indices" => tree.index = ints(p, most)?,
                "split_conditions" => tree.value = numbers(p, most)?,
                "default_left" => {
                    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
                    tree.default_left = numbers(p, most)?.into_iter().map(|v| v != 0.0).collect();
                }
                _ => p.skip_children(),
            }
            Ok(())
        })?;
        trees.push(tree);
    }
    Ok(trees)
}

fn ints(p: &mut Parser<'_>, most: usize) -> Result<Vec<i32>, ModelError> {
    // ALLOC: bounded by the definition, at most MAX_DEFINITION_BYTES (trees and depth are bounded too).
    Ok(numbers(p, most)?.into_iter().map(|v| v as i32).collect())
}
