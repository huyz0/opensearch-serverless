//! Whether a body may be stored for a kind, and the answer when it may not.

use oss_doc::mapping::json::{Json, parse, write_str};

use super::schema::{MAPPING_ONE_OF, Shape, keys, mandatory};
use super::{Answer, Kind};

/// The shortest password the plugin's default policy accepts.
pub const MIN_PASSWORD_CHARS: usize = 8;

fn error(status: u16, body: String) -> Answer {
    Answer { status, body }
}

fn invalid(
    // ALLOC: error path; one message.
    mut pieces: Vec<(&'static str, String)>,
) -> Answer {
    let mut out = String::from(r#"{"status":"error","reason":"Invalid configuration""#);
    for (name, keys) in pieces.drain(..) {
        out.push_str(",\"");
        out.push_str(name);
        out.push_str("\":{\"keys\":");
        write_str(&mut out, &keys);
        out.push('}');
    }
    out.push('}');
    error(400, out)
}

fn plain(reason: &str) -> Answer {
    // ALLOC: error path; one message.
    let mut out = String::from(r#"{"status":"error","reason":"#);
    write_str(&mut out, reason);
    out.push('}');
    error(400, out)
}

fn wrong_type(key: &str, expected: &str) -> Answer {
    // ALLOC: error path; one message.
    let mut out = String::from(r#"{"status":"error","reason":"Wrong datatype","#);
    write_str(&mut out, key);
    out.push(':');
    write_str(&mut out, expected);
    out.push('}');
    error(400, out)
}

/// The answer for a user created with neither `hash` nor `password`.
pub fn needs_credentials() -> Answer {
    error(
        400,
        r#"{"status":"BAD_REQUEST","message":"Please specify either 'hash' or 'password' when creating a new internal user."}"#
            // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
            .to_owned(),
    )
}

/// The answer for a password the policy refuses.
pub fn weak_password() -> Answer {
    error(
        400,
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        r#"{"status":"error","reason":"Weak password"}"#.to_owned(),
    )
}

/// Whether a password meets the policy.
pub fn password_ok(password: &str) -> bool {
    password.chars().count() >= MIN_PASSWORD_CHARS
}

/// Checks a request body for `kind` and returns it as the object to store.
///
/// # Errors
///
/// The plugin's 400: no body, a body that is not one JSON object, an
/// unknown key, a missing mandatory key, a mapping that names none of its
/// subjects, or a value of the wrong type.
pub fn validate(kind: Kind, body: &[u8]) -> Result<Json, Answer> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(plain("Request body required for this action."));
    }
    let Ok(doc @ Json::Obj(_)) = parse(body) else {
        return Err(plain("Could not parse content of request."));
    };
    let Json::Obj(members) = &doc else {
        return Err(plain("Could not parse content of request."));
    };
    let allowed = keys(kind);
    let unknown: Vec<&str> = members
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !allowed.iter().any(|(a, _)| a == k))
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        .collect();
    let has = |k: &str| members.iter().any(|(m, _)| m == k);
    let missing: Vec<&str> = mandatory(kind)
        .iter()
        .copied()
        .filter(|k| !has(k))
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        .collect();
    let one_of = kind == Kind::RolesMapping && !MAPPING_ONE_OF.iter().any(|k| has(k));
    // ALLOC: error path only; the vector allocates on its first push.
    let mut pieces = Vec::new();
    if !unknown.is_empty() {
        pieces.push(("invalid_keys", unknown.join(",")));
    }
    if !missing.is_empty() {
        pieces.push(("missing_mandatory_keys", missing.join(",")));
    }
    if one_of {
        pieces.push(("specify_one_of", MAPPING_ONE_OF.join(",")));
    }
    if !pieces.is_empty() {
        return Err(invalid(pieces));
    }
    for (key, value) in members {
        if let Some((_, shape)) = allowed.iter().find(|(a, _)| a == key) {
            check(key, value, *shape)?;
        }
    }
    Ok(doc)
}

fn is_strings(v: &Json) -> bool {
    matches!(v, Json::Arr(items) if items.iter().all(|i| matches!(i, Json::Str(_))))
}

fn check(key: &str, value: &Json, shape: Shape) -> Result<(), Answer> {
    match shape {
        Shape::Text if !matches!(value, Json::Str(_)) => Err(wrong_type(key, "String expected")),
        Shape::Strings if !is_strings(value) => Err(wrong_type(key, "Array expected")),
        Shape::Object if !matches!(value, Json::Obj(_)) => Err(wrong_type(key, "Object expected")),
        Shape::Entries(inner) => {
            let Json::Arr(items) = value else {
                return Err(wrong_type(key, "Array expected"));
            };
            for item in items {
                let Json::Obj(members) = item else {
                    return Err(wrong_type(key, "Object expected"));
                };
                let unknown: Vec<&str> = members
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .filter(|k| !inner.iter().any(|(a, _)| a == k))
                    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
                    .collect();
                if !unknown.is_empty() {
                    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
                    return Err(invalid(vec![("invalid_keys", unknown.join(","))]));
                }
                for (k, v) in members {
                    if let Some((_, shape)) = inner.iter().find(|(a, _)| a == k) {
                        check(k, v, *shape)?;
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// An internal user's body with its `password` (if it has one) replaced by
/// `hash`, the form that is stored.
pub fn with_hash(doc: Json, hash: Option<String>) -> Json {
    let Json::Obj(members) = doc else { return doc };
    let mut out: Vec<(String, Json)> = members
        .into_iter()
        .filter(|(k, _)| k != "password")
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        .collect();
    if let Some(hash) = hash {
        out.retain(|(k, _)| k != "hash");
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        out.push(("hash".to_owned(), Json::Str(hash)));
    }
    Json::Obj(out)
}
