//! The keys each metric's body declares, and `extended_stats`': `ExtendedStatsAggregationBuilder`'s
//! `ObjectParser` at OpenSearch 3.10.0 declares the values source's keys and
//! `sigma`, a double of at least 0.

use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};

/// The keys every metric here but `cardinality` declares
/// (`declareFields(PARSER, true, true, false)`: scriptable and formattable).
pub(super) const VALUES_SOURCE_KEYS: &[&str] =
    &["field", "missing", "value_type", "format", "script"];

/// `CardinalityAggregationBuilder`'s keys: `declareFields(PARSER, true,
/// false, false)`, not formattable, and its own three.
pub(super) const CARDINALITY_KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "script",
    "precision_threshold",
    "execution_hint",
    "rehash",
];

/// Every key `extended_stats` declares.
pub(super) const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "sigma",
];

/// `sigma`'s value, the parser on it: a number or numeric text.
pub(super) fn sigma(p: &Parser<'_>, kind: Kind, name: &str) -> Result<f64, DslError> {
    let failed = |cause: DslError| {
        DslError::xcontent(
            p.location(),
            "[extended_stats] failed to parse field [sigma]",
            Some(cause),
        )
    };
    let value = super::percentiles::double_value(kind, p.text()).map_err(failed)?;
    if value < 0.0 {
        // ALLOC: error path; one number's text.
        let mut found = Vec::new();
        super::double::write_java_double(&mut found, value)
            .map_err(|e| failed(DslError::illegal(e.to_string())))?;
        let found = String::from_utf8_lossy(&found).into_owned();
        return Err(failed(DslError::illegal(
            // ALLOC: error path; one message.
            format!("[sigma] must be greater than or equal to 0. Found [{found}] in [{name}]"),
        )));
    }
    Ok(value)
}
