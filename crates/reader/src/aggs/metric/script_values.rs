//! A metric's `script` in place of its field: the values `min`, `max`,
//! `sum`, `avg`, `stats` and `value_count` read are what a Painless script
//! returns for each document (`ValuesSource.Numeric.Script`), compiled by
//! this node and run by the engine. Served only when the node compiles
//! Painless ([`crate::painless_switch`]).

use std::sync::Arc;

use lucene_aggs::bucket_aggs::ScriptValuesSource;
use lucene_engine::painless_score::PainlessAggValues;
use lucene_engine::query_blob::Cursor;

use super::Metric;
use crate::dsl::error::DslError;
use crate::dsl::queries::function_score_script::{Lang, ScriptScore, object_with};
use crate::dsl::tokens::{Kind, Parser};
use crate::failure::Failure;
use crate::mapping::Mapping;
use crate::search::ShardError;

/// The most a script's source is, as core's `script.max_size_in_bytes`.
const MAX_SOURCE_BYTES: usize = 65_535;

/// Whether `metric` takes a script here.
pub(crate) fn takes_script(metric: Metric) -> bool {
    crate::painless_switch::native_compile()
        && matches!(
            metric,
            Metric::Min
                | Metric::Max
                | Metric::Sum
                | Metric::Avg
                | Metric::Stats
                | Metric::ValueCount
        )
}

/// A metric's `script` value: text, or an object, the parser on its token.
pub(crate) fn parse(p: &mut Parser<'_>, kind: Kind) -> Result<ScriptScore, DslError> {
    match kind {
        Kind::StartObject => object_with(p, true),
        Kind::String => {
            let source = p.text();
            if source.len() > MAX_SOURCE_BYTES {
                return Err(DslError::illegal(
                    "[script] source is over the 65535 byte limit",
                ));
            }
            Ok(ScriptScore {
                // ALLOC: bounded by the script's source, at most 65,535 bytes.
                source: source.to_owned(),
                lang: Lang::Painless,
                params: Vec::new(),
            })
        }
        _ => Err(DslError::illegal("[script] must be a string or an object")),
    }
}

/// The engine's values source for `script` over `mapping`.
pub(crate) fn values(
    mapping: &Mapping,
    script: &ScriptScore,
) -> Result<Arc<dyn ScriptValuesSource>, ShardError> {
    let refuse = |failure: Failure| ShardError {
        status: 400,
        failure,
    };
    let blob =
        crate::dsl::queries::function_score_painless::encode(mapping, script, 5).map_err(refuse)?;
    let values = PainlessAggValues::decode(&mut Cursor::new(&blob), false, None).map_err(|_| {
        refuse(Failure::plain(
            "script_exception",
            "the script could not be set up",
        ))
    })?;
    Ok(Arc::new(values))
}
