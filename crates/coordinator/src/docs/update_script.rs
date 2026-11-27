//! A scripted update's script: Painless over the document's `ctx`
//! (`_source`, `op` and the identity), by the Rust front end and runtime,
//! behind the native-compile switch.

use lucene_painless::ingest::run;
use lucene_painless::ir::Constant;
use oss_doc::mapping::json::{Json, parse};

use super::update::{ScriptOp, script_operation};
use crate::ingest::script::{to_constant, to_json};

/// What the script left: the operation it chose and the new source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The operation `ctx.op` names, as [`script_operation`] reads it.
    pub op: ScriptOp,
    /// The `_source` the script left, as JSON.
    pub source: Vec<u8>,
}

fn entry(key: &str, value: Constant) -> (Constant, Constant) {
    // ALLOC: one key a ctx entry; bounded by the fixed ctx.
    (Constant::Str(key.to_owned()), value)
}

/// Runs `source` with `params` over the document `doc` (the empty object for
/// a missing one) as an update script does: `ctx._source`, `ctx.op` (`index`
/// for a found document, `create` for a missing one) and the identity.
///
/// # Errors
///
/// The reason the update fails: the switch is off, `doc` is not an object, or
/// the script does not compile, is not run as Painless runs it, or throws.
pub fn run_update(
    source: &str,
    params: &Json,
    found: bool,
    index: &str,
    id: &str,
    version: i64,
    doc: &[u8],
) -> Result<Outcome, String> {
    if !oss_reader::painless_switch::native_compile() {
        // ALLOC: error path; one message.
        return Err("script updates need serverless.scripting.painless.native_compile".to_owned());
    }
    let Ok(body @ Json::Obj(_)) = parse(doc) else {
        // ALLOC: error path; one message.
        return Err("the document source is not a JSON object".to_owned());
    };
    // ALLOC: one ctx a script update; bounded by the document (at most the body limit).
    let ctx = Constant::Map(vec![
        entry("_index", Constant::Str(index.to_owned())),
        entry("_id", Constant::Str(id.to_owned())),
        entry("_version", Constant::Long(version)),
        // ALLOC: one short string a script update.
        entry(
            "op",
            Constant::Str(String::from(if found { "index" } else { "create" })),
        ),
        entry("_source", to_constant(&body)),
    ]);
    let left = run(source, &to_constant(params), &ctx)
        // ALLOC: error path; one message.
        .map_err(|e| format!("script_exception: {e}"))?;
    let Json::Obj(members) = to_json(&left)? else {
        // ALLOC: error path; one message.
        return Err("the script left a ctx that is not an object".to_owned());
    };
    let find = |k: &str| members.iter().find(|(n, _)| n == k).map(|(_, v)| v);
    let op = match find("op") {
        Some(Json::Str(s)) => script_operation(found, s),
        _ => ScriptOp::Noop,
    };
    let Some(Json::Obj(src)) = find("_source") else {
        // ALLOC: error path; one message.
        return Err("the script left a ctx without an object [_source]".to_owned());
    };
    Ok(Outcome {
        op,
        // ALLOC: one source a script update; bounded by the document.
        source: Json::Obj(src.clone()).to_json().into_bytes(),
    })
}
