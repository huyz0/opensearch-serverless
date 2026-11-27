//! The body of an update request and the merge of its `doc` into a source.

use oss_doc::mapping::json::{Json, parse};
use oss_rest::RestError;

/// A script of an update: its source and params.
pub(crate) struct ScriptDef {
    pub(crate) source: String,
    pub(crate) params: Json,
}

/// What an update body asks for.
pub(crate) struct UpdateBody {
    pub(crate) doc: Option<Json>,
    pub(crate) doc_as_upsert: bool,
    pub(crate) upsert: Option<Json>,
    pub(crate) script: Option<ScriptDef>,
    pub(crate) scripted_upsert: bool,
    pub(crate) detect_noop: bool,
}

fn flag(value: &Json, key: &str) -> Result<bool, RestError> {
    match value {
        Json::Bool(b) => Ok(*b),
        // ALLOC: error path; one message.
        _ => Err(RestError::illegal_argument(format!(
            "[{key}] must be a boolean"
        ))),
    }
}

fn object(value: &Json, key: &str) -> Result<Json, RestError> {
    match value {
        Json::Obj(_) => Ok(value.clone()),
        // ALLOC: error path; one message.
        _ => Err(RestError::illegal_argument(format!(
            "[{key}] must be an object"
        ))),
    }
}

pub(crate) fn script(value: &Json) -> Result<ScriptDef, RestError> {
    let Json::Obj(members) = value else {
        return Err(RestError::illegal_argument("[script] must be an object"));
    };
    let mut source = None;
    let mut params = Json::Obj(Vec::new());
    for (key, v) in members {
        match (key.as_str(), v) {
            // ALLOC: one script source; bounded by the request body.
            ("source", Json::Str(s)) => source = Some(s.clone()),
            ("lang", Json::Str(l)) if l == "painless" => {}
            ("lang", _) => {
                return Err(RestError::illegal_argument(
                    "update scripts run only lang [painless]",
                ));
            }
            ("params", Json::Obj(_)) => params = v.clone(),
            // ALLOC: error path; one message.
            (other, _) => {
                return Err(RestError::illegal_argument(format!(
                    "[script] does not support [{other}]"
                )));
            }
        }
    }
    let source = source.ok_or_else(|| RestError::illegal_argument("script [source] is missing"))?;
    Ok(ScriptDef { source, params })
}

/// Reads the body of an update.
///
/// # Errors
///
/// A body that is not an object, a key this route does not serve, or a value
/// of the wrong type.
pub(crate) fn read(bytes: &[u8]) -> Result<UpdateBody, RestError> {
    let Ok(Json::Obj(members)) = parse(bytes) else {
        return Err(RestError::illegal_argument(
            "the update body is not a JSON object",
        ));
    };
    let mut body = UpdateBody {
        doc: None,
        doc_as_upsert: false,
        upsert: None,
        script: None,
        scripted_upsert: false,
        detect_noop: true,
    };
    for (key, v) in &members {
        match key.as_str() {
            "doc" => body.doc = Some(object(v, "doc")?),
            "upsert" => body.upsert = Some(object(v, "upsert")?),
            "doc_as_upsert" => body.doc_as_upsert = flag(v, key)?,
            "scripted_upsert" => body.scripted_upsert = flag(v, key)?,
            "detect_noop" => body.detect_noop = flag(v, key)?,
            "script" => body.script = Some(script(v)?),
            other => {
                // ALLOC: error path; one message.
                return Err(RestError::illegal_argument(format!(
                    "[UpdateRequest] unknown field [{other}]"
                )));
            }
        }
    }
    Ok(body)
}

/// Merges `from` into `into`: objects key by key, anything else replaces.
pub(crate) fn merge(into: &mut Json, from: &Json) {
    let (Json::Obj(have), Json::Obj(add)) = (&mut *into, from) else {
        // ALLOC: one value of the merged document; bounded by the request body.
        *into = from.clone();
        return;
    };
    for (key, v) in add {
        match have.iter_mut().find(|(k, _)| k == key) {
            Some((_, slot)) => merge(slot, v),
            // ALLOC: one member of the merged document; bounded by the request body.
            None => have.push((key.clone(), v.clone())),
        }
    }
}
