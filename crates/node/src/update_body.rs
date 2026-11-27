//! The body of an update request and the merge of its `doc` into a source.

use oss_doc::mapping::json::{Json, parse};
use oss_rest::RestError;
use oss_types::stored_scripts::StoredScripts;

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

pub(crate) fn script(
    value: &Json,
    scripts: Option<&dyn StoredScripts>,
) -> Result<ScriptDef, RestError> {
    let Json::Obj(members) = value else {
        return Err(RestError::illegal_argument("[script] must be an object"));
    };
    let mut source = None;
    let mut id = None;
    let mut has_lang = false;
    let mut params = Json::Obj(Vec::new());
    for (key, v) in members {
        match (key.as_str(), v) {
            // ALLOC: one id; bounded by the request body.
            ("id", Json::Str(s)) => id = Some(s.clone()),
            ("id", _) => {
                return Err(RestError::illegal_argument(
                    "[script] [id] must be a string",
                ));
            }
            // ALLOC: one script source; bounded by the request body.
            ("source", Json::Str(s)) => source = Some(s.clone()),
            ("lang", Json::Str(l)) if l == "painless" => has_lang = true,
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
    if let Some(id) = id {
        if source.is_some() || has_lang {
            return Err(RestError::illegal_argument(
                "a stored script [id] takes no [source] or [lang]",
            ));
        }
        return Ok(ScriptDef {
            source: stored(&id, scripts)?,
            params,
        });
    }
    let source = source.ok_or_else(|| RestError::illegal_argument("script [source] is missing"))?;
    Ok(ScriptDef { source, params })
}

/// The source of the stored Painless script `id`.
fn stored(id: &str, scripts: Option<&dyn StoredScripts>) -> Result<String, RestError> {
    use oss_types::stored_scripts::ResolveError;
    let Some(scripts) = scripts else {
        return Err(RestError::illegal_argument(
            "a stored script in [script] is not served on this node: send the script as [source]",
        ));
    };
    match scripts.resolve(id) {
        Ok(found) if found.lang == "painless" => Ok(found.source),
        Ok(found) => Err(RestError::illegal_argument(format!(
            // ALLOC: error path; one message.
            "the stored script [{id}] is lang [{}]: update scripts run only lang [painless]",
            found.lang
        ))),
        Err(ResolveError::Missing) => Err(RestError::exception(
            404,
            oss_rest::Exception::new(
                "resource_not_found_exception",
                // ALLOC: error path; one message.
                format!("unable to find script [{id}] in cluster state"),
            ),
        )),
        Err(ResolveError::TooBig(m)) => Err(RestError::simple(429, m)),
        Err(ResolveError::Failed(m)) => Err(RestError::simple(500, m)),
    }
}

/// Reads the body of an update.
///
/// # Errors
///
/// A body that is not an object, a key this route does not serve, or a value
/// of the wrong type.
pub(crate) fn read(
    bytes: &[u8],
    scripts: Option<&dyn StoredScripts>,
) -> Result<UpdateBody, RestError> {
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
            "script" => body.script = Some(script(v, scripts)?),
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

#[cfg(test)]
mod tests {
    use super::*;
    use oss_types::stored_scripts::{ResolveError, Stored};

    struct One;

    impl StoredScripts for One {
        fn resolve(&self, id: &str) -> Result<Stored, ResolveError> {
            match id {
                "p" => Ok(Stored {
                    lang: "painless".into(),
                    source: "ctx._source.n = 1".into(),
                }),
                "m" => Ok(Stored {
                    lang: "mustache".into(),
                    source: "{}".into(),
                }),
                _ => Err(ResolveError::Missing),
            }
        }
    }

    // docs/specs/stored-script-ids AC-3
    #[test]
    fn an_update_body_takes_a_stored_script_by_id() {
        let body = read(
            br#"{"script":{"id":"p","params":{"a":1}}}"#,
            Some(&One as &dyn StoredScripts),
        )
        .expect("reads");
        let def = body.script.expect("script");
        assert_eq!(def.source, "ctx._source.n = 1");
        assert!(matches!(def.params, Json::Obj(ref m) if m.len() == 1));
    }

    // docs/specs/stored-script-ids AC-4
    #[test]
    fn what_a_stored_id_may_not_be_is_refused() {
        let scripts = Some(&One as &dyn StoredScripts);
        for (body, status) in [
            (r#"{"script":{"id":"nope"}}"#, 404),
            (r#"{"script":{"id":"m"}}"#, 400),
            (r#"{"script":{"id":"p","source":"1"}}"#, 400),
            (r#"{"script":{"id":"p","lang":"painless"}}"#, 400),
            (r#"{"script":{"id":3}}"#, 400),
        ] {
            let error = read(body.as_bytes(), scripts).err().expect(body);
            assert_eq!(error.status(), status, "{body}");
        }
        let none = read(br#"{"script":{"id":"p"}}"#, None)
            .err()
            .expect("no resolver");
        assert_eq!(none.status(), 400);
    }
}
