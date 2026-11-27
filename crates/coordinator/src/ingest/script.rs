//! The `script` processor: Painless source run over a document by the Rust
//! front end and runtime, behind the native-compile switch.

use lucene_painless::ingest::run;
use lucene_painless::ir::Constant;
use oss_doc::mapping::json::{Json, is_integral, java_double};

/// A script processor's source and its params.
#[derive(Debug, Clone)]
pub(crate) struct Script {
    pub(super) source: String,
    pub(super) params: Constant,
}

/// A JSON value as the script sees it.
pub(super) fn to_constant(v: &Json) -> Constant {
    match v {
        Json::Null => Constant::Null,
        Json::Bool(b) => Constant::Bool(*b),
        Json::Num(n) => {
            if is_integral(n) {
                match n.parse::<i32>() {
                    Ok(i) => Constant::Int(i),
                    Err(_) => n.parse::<i64>().map_or_else(
                        |_| Constant::Double(n.parse().unwrap_or(0.0)),
                        Constant::Long,
                    ),
                }
            } else {
                Constant::Double(n.parse().unwrap_or(0.0))
            }
        }
        Json::Str(s) => Constant::Str(s.clone()),
        // ALLOC: one document's working copy; bounded by the source (at most the body limit).
        Json::Arr(items) => Constant::List(items.iter().map(to_constant).collect()),
        Json::Obj(members) => Constant::Map(
            members
                .iter()
                // ALLOC: one document's working copy; bounded by the source (at most the body limit).
                .map(|(k, v)| (Constant::Str(k.clone()), to_constant(v)))
                .collect(),
        ),
    }
}

/// A value the script left, as JSON.
fn to_json(c: &Constant) -> Result<Json, String> {
    Ok(match c {
        Constant::Null => Json::Null,
        Constant::Bool(b) => Json::Bool(*b),
        // ALLOC: one number of one document; bounded by the source (at most the body limit).
        Constant::Byte(n) => Json::Num(n.to_string()),
        // ALLOC: one value of one document; bounded by the source (at most the body limit).
        Constant::Short(n) => Json::Num(n.to_string()),
        // ALLOC: one value of one document; bounded by the source (at most the body limit).
        Constant::Int(n) => Json::Num(n.to_string()),
        // ALLOC: one value of one document; bounded by the source (at most the body limit).
        Constant::Long(n) => Json::Num(n.to_string()),
        // ALLOC: one number of one document; bounded by the source (at most the body limit).
        Constant::Float(n) => Json::Num(java_double(f64::from(*n))),
        // ALLOC: one value of one document; bounded by the source (at most the body limit).
        Constant::Double(n) => Json::Num(java_double(*n)),
        // ALLOC: one character of one document; bounded by the source (at most the body limit).
        Constant::Char(n) => Json::Str(String::from(
            char::from_u32(u32::from(*n)).unwrap_or('\u{fffd}'),
        )),
        // ALLOC: one value of one document; bounded by the source (at most the body limit).
        Constant::Str(s) => Json::Str(s.clone()),
        // ALLOC: one value of one document; bounded by the source (at most the body limit).
        Constant::List(items) => Json::Arr(items.iter().map(to_json).collect::<Result<_, _>>()?),
        Constant::Map(entries) => Json::Obj(
            entries
                .iter()
                .map(|(k, v)| match k {
                    Constant::Str(k) => Ok((k.clone(), to_json(v)?)),
                    // ALLOC: error path; one message.
                    _ => Err("the script left a field name that is not a string".to_owned()),
                })
                .collect::<Result<_, _>>()?,
        ),
        Constant::Pattern(..) => {
            // ALLOC: error path; one message.
            return Err("the script left a value a document cannot hold".to_owned());
        }
    })
}

/// Runs the script over `doc`, replacing it with the document the script leaves.
pub(super) fn run_script(script: &Script, doc: &mut Json) -> Result<(), String> {
    let left = run(&script.source, &script.params, &to_constant(doc))
        // ALLOC: error path; one message.
        .map_err(|e| format!("script_exception: {e}"))?;
    *doc = to_json(&left)?;
    Ok(())
}
