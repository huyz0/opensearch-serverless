//! `script_fields` (`ScriptFieldsContext`, `ScriptFieldsPhase`) over
//! `lang: expression`: each hit's `fields` carries the script's number.

use std::io;
use std::sync::Arc;

use lucene_engine::painless_score::{Leaf, scripts};
use lucene_search::directory_reader::SegmentReader;

use crate::dsl::SearchSource;
use crate::dsl::error::DslError;
use crate::dsl::queries::function_score_script::{self, ScriptScore};
use crate::dsl::tokens::{Kind, Parser};
use crate::fetch::docvalue_fields::{HitField, Value, ValueKind};
use crate::mapping::Mapping;
use crate::search::ShardError;

/// `index.max_script_fields`' default.
pub const MAX_SCRIPT_FIELDS: usize = 32;

/// The `script_fields` object, the parser on its start.
///
/// # Errors
///
/// A [`DslError`] for an entry that is not `{"script": {..}}`, or that
/// this node does not serve.
pub fn parse(p: &mut Parser<'_>) -> Result<Vec<(String, ScriptScore)>, DslError> {
    // ALLOC: one entry a script field, bounded by the body's recording.
    let mut out = Vec::new();
    while p.next_token() == Some(Kind::FieldName) {
        // ALLOC: one name a script field, bounded by the body's recording.
        let name = p.text().to_owned();
        if p.next_token() != Some(Kind::StartObject) {
            return Err(DslError::illegal(
                "[script_fields] each field takes an object with a [script]",
            ));
        }
        let mut script = None;
        while p.next_token() == Some(Kind::FieldName) {
            // ALLOC: bounded by the request body's recording.
            let key = p.text().to_owned();
            match (key.as_str(), p.next_token()) {
                ("script", Some(Kind::StartObject)) => {
                    script = Some(function_score_script::object(p)?);
                }
                ("script", Some(Kind::String)) => {
                    return Err(DslError::illegal(
                        "[script_fields] runs only lang [expression] on this node, and a script given as text is painless",
                    ));
                }
                ("ignore_failure", _) => {
                    return Err(DslError::unsupported("[script_fields] [ignore_failure]"));
                }
                _ => {
                    // ALLOC: error path; one message.
                    return Err(DslError::illegal(format!(
                        "[script_fields] unknown field [{key}]"
                    )));
                }
            }
        }
        let Some(script) = script else {
            return Err(DslError::illegal("[script_fields] requires a [script]"));
        };
        out.push((name, script));
    }
    if out.len() > MAX_SCRIPT_FIELDS {
        // ALLOC: bounded by the request body's recording.
        return Err(DslError::illegal(format!(
            "Trying to retrieve too many script_fields. Must be less than or equal to: [{MAX_SCRIPT_FIELDS}] but was [{}]. This limit can be set by changing the [index.max_script_fields] index level setting.",
            out.len()
        )));
    }
    Ok(out)
}

/// The `docvalue_fields` then the script fields, resolved over `mapping`.
///
/// # Errors
///
/// A shard failure from either.
pub fn all(source: &SearchSource, mapping: &Mapping) -> Result<Vec<HitField>, ShardError> {
    let (docvalues, scripts) = (&source.docvalue_fields, &source.script_fields);
    if !source.fields.is_empty() {
        crate::fetch::source_fields::check_alone(
            !docvalues.is_empty() || !scripts.is_empty() || source.collapse.is_some(),
        )?;
        return crate::fetch::source_fields::resolve(&source.fields, mapping);
    }
    // ALLOC: one entry an asked doc value field, at most MAX_FIELDS.
    let mut asked = docvalues.clone();
    if let Some(c) = &source.collapse
        && !asked.iter().any(|d| d.field == c.field)
    {
        // The collapse field's values ride the hit as a doc value field.
        asked.push(crate::fetch::docvalue_fields::DocValueField {
            // ALLOC: one name a collapse.
            field: c.field.clone(),
            format: None,
        });
    }
    let mut fields = if asked.is_empty() {
        // ALLOC: bounded by the request body's recording.
        Vec::new()
    } else {
        crate::fetch::docvalue_fields::resolve(&asked, mapping)?
    };
    fields.extend(resolve(scripts, mapping)?);
    Ok(fields)
}

/// Each entry as a field the hit's `fields` object writes.
///
/// # Errors
///
/// A shard failure for a script that does not build over `mapping`.
pub fn resolve(
    scripts: &[(String, ScriptScore)],
    mapping: &Mapping,
) -> Result<Vec<HitField>, ShardError> {
    // ALLOC: one entry a script field, at most MAX_SCRIPT_FIELDS.
    let mut out = Vec::with_capacity(scripts.len());
    for (name, script) in scripts {
        let function =
            function_score_script::field(mapping, script).map_err(ShardError::bad_request)?;
        out.push(HitField {
            // ALLOC: one name a script field, at most MAX_SCRIPT_FIELDS.
            name: name.clone(),
            kind: ValueKind::Script,
            times: 1,
            script: Some(function),
            source: None,
        });
    }
    Ok(out)
}

/// The script's number for `doc` of `segment`.
pub(crate) fn values(
    field: &HitField,
    segment: &SegmentReader,
    doc: i32,
) -> io::Result<Vec<Value>> {
    let Some(function) = &field.script else {
        // ALLOC: bounded by the request body's recording.
        return Ok(Vec::new());
    };
    let mut leaf =
        scripts(&Arc::clone(&function.0), segment, Leaf::Ord(0), None).map_err(io::Error::other)?;
    leaf.env.doc = doc;
    leaf.env.score = 0.0;
    let value = leaf.run_value().map_err(io::Error::other)?;
    // ALLOC: one value a script field.
    Ok(vec![Value::Double(value)])
}
