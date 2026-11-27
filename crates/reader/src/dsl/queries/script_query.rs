//! The `script` query over `lang: painless`: documents the script returns
//! `true` for. `ScriptQueryBuilder.fromXContent`, served when the node
//! compiles Painless ([`crate::painless_switch`]); the engine's filter node
//! runs it.

use lucene_engine::painless_score::decode_filter;
use lucene_engine::query_blob::Cursor;
use lucene_search::Clause;

use super::function_score_script::{Lang, ScriptScore, object_with};
use super::{More, function_score_painless};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::float_value;
use crate::failure::Failure;
use crate::lucene_query::QueryContext;

/// The most a script's source is, as core's `script.max_size_in_bytes`.
const MAX_SOURCE_BYTES: usize = 65_535;

/// `ScriptQueryBuilder.fromXContent`: on the query object's start.
pub fn parse(p: &mut Parser<'_>) -> Result<More, DslError> {
    let (mut script, mut boost) = (None, 1.0);
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: bounded by the script's source, at most 65,535 bytes.
        let name = p.text().to_owned();
        match (name.as_str(), p.next_token()) {
            ("script", Some(Kind::StartObject)) => script = Some(object_with(p, true)?),
            ("script", Some(Kind::String)) => {
                let source = p.text();
                if source.len() > MAX_SOURCE_BYTES {
                    return Err(DslError::illegal(
                        "[script] source is over the 65535 byte limit",
                    ));
                }
                script = Some(ScriptScore {
                    // ALLOC: bounded by the script's source, at most 65,535 bytes.
                    source: source.to_owned(),
                    lang: Lang::Painless,
                    params: Vec::new(),
                });
            }
            ("boost", Some(_)) => boost = float_value(p)?,
            ("_name", _) => return Err(DslError::unsupported("[script] [_name]")),
            _ => {
                // ALLOC: error path; one message.
                return Err(DslError::illegal(format!(
                    "[script] query does not support [{name}]"
                )));
            }
        }
    }
    let script =
        script.ok_or_else(|| DslError::illegal("script must be provided with a [script]"))?;
    if script.lang != Lang::Painless {
        return Err(DslError::illegal(
            "[script] query runs only lang [painless]",
        ));
    }
    // ALLOC: one box a script query.
    Ok(More::Script(Box::new(script), boost))
}

/// The engine's filter for `script` over the mapping in `cx`.
pub fn build(cx: &QueryContext<'_>, script: &ScriptScore) -> Result<Clause, Failure> {
    let blob = function_score_painless::encode(cx.mapping, script, 2)?;
    decode_filter(&mut Cursor::new(&blob))
        .map_err(|_| Failure::plain("script_exception", "the script could not be set up"))
}
