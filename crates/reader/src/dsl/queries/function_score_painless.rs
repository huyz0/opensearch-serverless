//! `script_score` over `lang: painless`: the source goes to the engine as it
//! is written, where the Rust front end compiles it and the engine checks it
//! runs exactly as Painless does. See docs/specs/painless-script-score.

use lucene_engine::painless_score::{
    FieldKind, SOURCE_SCRIPT, SourceError, fields_of, source_spec,
};
use lucene_painless::ir::{Constant, encode_constant};

use super::function_score_script::ScriptScore;
use crate::failure::Failure;
use crate::mapping::{FieldType, Mapping};

fn script_error(reason: String) -> Failure {
    Failure::plain("script_exception", reason)
}

fn source_failure(e: SourceError) -> Failure {
    match e {
        SourceError::Compile { offset, message } => {
            // ALLOC: error path; one message.
            script_error(format!("compile error: {message} at offset {offset}"))
        }
        SourceError::Unsupported(what) => {
            // ALLOC: error path; one message.
            script_error(format!(
                "this node does not serve this Painless script yet: {what}"
            ))
        }
        SourceError::Refused(why) => {
            // ALLOC: error path; one message.
            script_error(format!(
                "this node does not run this Painless script as Painless does: {why}"
            ))
        }
    }
}

/// How a script reads `field` of `mapping`.
fn kind_of(mapping: &Mapping, field: &str) -> FieldKind {
    let Some(f) = mapping.get(field) else {
        return FieldKind::Unmapped;
    };
    if !f.doc_values {
        return FieldKind::Unsupported;
    }
    match f.ty {
        FieldType::Int { .. } | FieldType::Long => FieldKind::Long,
        FieldType::Date => FieldKind::Date,
        FieldType::Keyword => FieldKind::Keyword,
        FieldType::Double => FieldKind::Double,
        FieldType::Float => FieldKind::Float,
        FieldType::Text | FieldType::Unsupported(_) => FieldKind::Unsupported,
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    let n = i32::try_from(b.len()).unwrap_or(i32::MAX);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(b);
}

fn kind_byte(k: FieldKind) -> u8 {
    match k {
        FieldKind::Long => 0,
        FieldKind::Double => 1,
        FieldKind::Float => 2,
        FieldKind::HalfFloat => 3,
        FieldKind::Boolean => 4,
        FieldKind::Keyword => 5,
        FieldKind::Date => 6,
        FieldKind::DateNanos => 7,
        FieldKind::Unsupported => 254,
        FieldKind::Unmapped => 255,
    }
}

/// The engine's blob for the Painless `script` as a script of engine kind
/// `kind` (`0` score), refused with its reason when the engine would not run
/// it as Painless does.
pub(crate) fn encode(
    mapping: &Mapping,
    script: &ScriptScore,
    kind: u8,
) -> Result<Vec<u8>, Failure> {
    let names = fields_of(&script.source, kind).map_err(source_failure)?;
    // ALLOC: one entry a field the script reads, bounded by the source.
    let fields: Vec<(String, String, FieldKind)> = names
        .into_iter()
        .map(|n| {
            let k = kind_of(mapping, &n);
            // ALLOC: one name a field the script reads, bounded by the source.
            (n.clone(), n, k)
        })
        .collect();
    // ALLOC: one entry a param, bounded by the request body.
    let mut entries = Vec::with_capacity(script.params.len());
    for (k, v) in &script.params {
        // ALLOC: one name a param, bounded by the request body.
        entries.push((Constant::Str(k.clone()), v.constant()));
    }
    let params = Constant::Map(entries);
    // ALLOC: one copy of the params and of the fields, bounded by the request body.
    source_spec(kind, &script.source, params.clone(), "", 0, fields.clone())
        .map_err(source_failure)?;
    // ALLOC: one blob a script, bounded by the source and its fields.
    let mut blob = vec![SOURCE_SCRIPT | kind];
    put_bytes(&mut blob, script.source.as_bytes());
    put_bytes(&mut blob, &encode_constant(&params));
    put_bytes(&mut blob, b"");
    blob.extend_from_slice(&0i32.to_le_bytes());
    blob.extend_from_slice(&0i32.to_le_bytes());
    blob.extend_from_slice(&i32::try_from(fields.len()).unwrap_or(0).to_le_bytes());
    for (name, read, k) in &fields {
        put_bytes(&mut blob, name.as_bytes());
        put_bytes(&mut blob, read.as_bytes());
        blob.push(kind_byte(*k));
    }
    Ok(blob)
}
