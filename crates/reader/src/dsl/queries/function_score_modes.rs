//! The enum names `function_score` reads (`ScoreMode`, `CombineFunction`,
//! `FieldValueFactorFunction.Modifier`, `MultiValueMode`), each as Java's
//! `valueOf` of the upper-cased text, with its failure.

use lucene_engine::function_score::{Combine, Modifier, MultiValue, ScoreMode};

use crate::dsl::error::DslError;

/// `Enum.valueOf(text.toUpperCase(Locale.ROOT))`'s failure.
fn no_constant(class: &str, text: &str) -> DslError {
    DslError::illegal(
        // ALLOC: error path; one message.
        format!("No enum constant {class}.{}", text.to_uppercase()),
    )
}

pub(super) fn score_mode(text: &str) -> Result<ScoreMode, DslError> {
    Ok(match text.to_uppercase().as_str() {
        "MULTIPLY" => ScoreMode::Multiply,
        "FIRST" => ScoreMode::First,
        "AVG" => ScoreMode::Avg,
        "MAX" => ScoreMode::Max,
        "SUM" => ScoreMode::Sum,
        "MIN" => ScoreMode::Min,
        _ => {
            return Err(no_constant(
                "org.opensearch.common.lucene.search.function.FunctionScoreQuery.ScoreMode",
                text,
            ));
        }
    })
}

pub(super) fn boost_mode(text: &str) -> Result<Combine, DslError> {
    Ok(match text.to_uppercase().as_str() {
        "MULTIPLY" => Combine::Multiply,
        "REPLACE" => Combine::Replace,
        "SUM" => Combine::Sum,
        "AVG" => Combine::Avg,
        "MIN" => Combine::Min,
        "MAX" => Combine::Max,
        _ => {
            return Err(no_constant(
                "org.opensearch.common.lucene.search.function.CombineFunction",
                text,
            ));
        }
    })
}

pub(super) fn modifier(text: &str) -> Result<Modifier, DslError> {
    Ok(match text.to_uppercase().as_str() {
        "NONE" => Modifier::None,
        "LOG" => Modifier::Log,
        "LOG1P" => Modifier::Log1p,
        "LOG2P" => Modifier::Log2p,
        "LN" => Modifier::Ln,
        "LN1P" => Modifier::Ln1p,
        "LN2P" => Modifier::Ln2p,
        "SQUARE" => Modifier::Square,
        "SQRT" => Modifier::Sqrt,
        "RECIPROCAL" => Modifier::Reciprocal,
        _ => {
            return Err(no_constant(
                "org.opensearch.common.lucene.search.function.FieldValueFactorFunction.Modifier",
                text,
            ));
        }
    })
}

pub(super) fn multi_value(text: &str) -> Result<MultiValue, DslError> {
    Ok(match text.to_uppercase().as_str() {
        "MIN" => MultiValue::Min,
        "MAX" => MultiValue::Max,
        "AVG" => MultiValue::Avg,
        "SUM" => MultiValue::Sum,
        "MEDIAN" => MultiValue::Median,
        _ => {
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!("Illegal sort mode: {text}"),
            ));
        }
    })
}
