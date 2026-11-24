//! What the repository write routes decide: the reserved names, the request
//! body of a registration, which names a delete takes and how a removal and a
//! verification are answered.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/snapshot/RepositoryHandler.java`
//! (ADR 0040). The name rules are [`crate::snapshot_admin::check_name`]'s.

use std::fmt::Write;

use oss_doc::mapping::json::{Json, parse, write_str};
use oss_format::{NATIVE_TYPE, RepositoryRecord};

use crate::route::resolve::Refusal;
use crate::snapshot_admin::check_name;

/// The answer to a registration or a removal.
pub const ACKNOWLEDGED: &str = r#"{"acknowledged":true}"#;

fn refuse(status: u16, kind: &'static str, reason: String) -> Refusal {
    Refusal {
        status,
        kind,
        reason,
    }
}

/// The 501 for a name starting with `_` other than `_all`: the shell reserves
/// those for APIs.
///
/// # Errors
///
/// The refusal.
pub fn check_not_reserved(name: &str) -> Result<(), Refusal> {
    if name.starts_with('_') && name != "_all" {
        return Err(refuse(
            501,
            "not_implemented",
            // ALLOC: error path; one message.
            "repository names beginning with an underscore are reserved for APIs".to_owned(),
        ));
    }
    Ok(())
}

/// The record a `PUT` or `POST /_snapshot/{name}` stores, from its body
/// (`None` when there is none), in the shell's order: the reserved names,
/// the name, then the body (a `type` other than `fs`, `crypto_settings`,
/// settings flattened to text).
///
/// # Errors
///
/// 501 `not_implemented`, 400 `invalid_snapshot_name_exception`, 400
/// `malformed_body` for a body that is not an object, 400
/// `illegal_argument_exception` for a setting that is null or an array,
/// 501 `unsupported_repository_type` and 501 `unsupported_repository_setting`.
pub fn registration(
    name: &str,
    body: Option<&[u8]>,
    now_millis: i64,
) -> Result<RepositoryRecord, Refusal> {
    check_not_reserved(name)?;
    check_name(name, "repository")?;
    let members = match body.filter(|b| !b.is_empty()) {
        // ALLOC: one name or value, bounded by the 1 MiB body.
        None => Vec::new(),
        Some(bytes) => match parse(bytes) {
            Ok(Json::Obj(members)) => members,
            _ => {
                return Err(refuse(
                    400,
                    "malformed_body",
                    // ALLOC: error path; one message.
                    "could not parse the repository: the body is not a JSON object".to_owned(),
                ));
            }
        },
    };
    // ALLOC: one name or value, bounded by the 1 MiB body.
    let mut repo_type = NATIVE_TYPE.to_owned();
    let mut crypto = false;
    // ALLOC: one entry a setting leaf, bounded by the 1 MiB body.
    let mut settings: Vec<(String, String)> = Vec::new();
    for (key, value) in &members {
        match (key.as_str(), value) {
            ("type", Json::Str(t)) => t.clone_into(&mut repo_type),
            ("type", Json::Null) => {}
            ("type", _) => {
                return Err(bad("type must be a string"));
            }
            ("crypto_settings", Json::Null) => {}
            ("crypto_settings", _) => crypto = true,
            ("settings", Json::Obj(inner)) => {
                for (k, v) in inner {
                    flatten(k, v, &mut settings)?;
                }
            }
            ("settings", Json::Null) => {}
            ("settings", _) => return Err(bad("settings must be an object")),
            _ => {}
        }
    }
    if repo_type != NATIVE_TYPE {
        return Err(refuse(
            501,
            "unsupported_repository_type",
            // ALLOC: error path; one message.
            format!(
                "type '{repo_type}' is not supported; this deployment has exactly one object store, \
                 reached with type '{NATIVE_TYPE}'"
            ),
        ));
    }
    if crypto {
        return Err(refuse(
            501,
            "unsupported_repository_setting",
            // ALLOC: error path; one message.
            "repository crypto_settings are not supported".to_owned(),
        ));
    }
    Ok(RepositoryRecord {
        // ALLOC: one name or value, bounded by the 1 MiB body.
        name: name.to_owned(),
        repo_type,
        created_at_millis: now_millis,
        settings,
    })
}

fn bad(reason: &str) -> Refusal {
    // ALLOC: error path; one message.
    refuse(400, "illegal_argument_exception", reason.to_owned())
}

/// A setting as the shell's flat `Settings` writes it: nested objects join
/// their keys with `.`, every scalar is text. A null or an array is refused,
/// because the record's readers take scalars only.
fn flatten(key: &str, value: &Json, out: &mut Vec<(String, String)>) -> Result<(), Refusal> {
    let text = match value {
        Json::Obj(members) => {
            for (name, inner) in members {
                // ALLOC: one dotted key, bounded by the body.
                flatten(&format!("{key}.{name}"), inner, out)?;
            }
            return Ok(());
        }
        // ALLOC: one name or value, bounded by the 1 MiB body.
        Json::Bool(b) => b.to_string(),
        Json::Num(text) | Json::Str(text) => text.clone(),
        Json::Null | Json::Arr(_) => {
            return Err(refuse(
                400,
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!("repository setting [{key}] must be a string, number or boolean"),
            ));
        }
    };
    // ALLOC: one entry, bounded by the body.
    out.retain(|(k, _)| k != key);
    // ALLOC: one entry, bounded by the body.
    out.push((key.to_owned(), text));
    Ok(())
}

/// The names a `DELETE /_snapshot/{names}` takes: the list split on commas,
/// every one validated before any is deleted.
///
/// # Errors
///
/// 400 `invalid_snapshot_name_exception` for a name `validateId` refuses
/// (`_all`, `_x` and every pattern among them).
pub fn deletion(names: &str) -> Result<Vec<&str>, Refusal> {
    // ALLOC: one name each, bounded by the path.
    let list: Vec<&str> = names.split(',').collect();
    for name in &list {
        check_name(name, "repository")?;
        if name.contains('*') {
            return Err(bad(
                "repository mutations require concrete repository names",
            ));
        }
    }
    Ok(list)
}

/// The 404 for a repository that is not registered.
#[must_use]
pub fn missing(name: &str) -> Refusal {
    crate::snapshot_admin::repository_missing(name)
}

/// The 409 for a repository a snapshot still names.
#[must_use]
pub fn in_use(name: &str) -> Refusal {
    refuse(
        409,
        "repository_in_use",
        // ALLOC: error path; one message.
        format!("repository [{name}] still holds snapshots"),
    )
}

/// The 500 for a repository with more snapshot markers than a delete checks.
#[must_use]
pub fn too_many_snapshots_to_check(name: &str) -> Refusal {
    refuse(
        500,
        "too_many_snapshots_to_check",
        // ALLOC: error path; one message.
        format!("repository [{name}] has too many snapshots to check before it is removed"),
    )
}

/// The 500 for a repository record that does not parse.
#[must_use]
pub fn unreadable(name: &str) -> Refusal {
    refuse(
        500,
        "repository_unreadable",
        // ALLOC: error path; one message.
        format!("repository [{name}] could not be read"),
    )
}

/// The answer to a verification: this node as the only node.
#[must_use]
pub fn render_verify(node_id: &str, node_name: &str) -> String {
    // ALLOC: one answer, once a request.
    let mut out = String::from(r#"{"nodes":{"#);
    write_str(&mut out, node_id);
    out.push_str(r#":{"name":"#);
    write_str(&mut out, node_name);
    let _ = out.write_str("}}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(name: &str, body: &str) -> Result<RepositoryRecord, Refusal> {
        registration(name, Some(body.as_bytes()), 7)
    }

    #[test]
    fn names_are_refused_in_the_shells_order() {
        assert_eq!(put("_x", "{}").unwrap_err().status, 501);
        assert_eq!(put("_all", "{}").unwrap_err().status, 400);
        assert_eq!(put("a/b", "{}").unwrap_err().status, 400);
        assert_eq!(
            put("_x", "nonsense").unwrap_err().kind,
            "not_implemented",
            "the reserved name is refused before the body is read"
        );
    }

    #[test]
    fn a_registration_defaults_its_type_and_flattens_settings_to_text() {
        let r = put(
            "backups",
            r#"{"settings":{"location":"x","max":5,"on":true,"a":{"b":"c"}}}"#,
        )
        .expect("reads");
        assert_eq!(
            (r.name.as_str(), r.repo_type.as_str(), r.created_at_millis),
            ("backups", "fs", 7)
        );
        let got: Vec<(&str, &str)> = r
            .settings
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("location", "x"),
                ("max", "5"),
                ("on", "true"),
                ("a.b", "c")
            ]
        );
        let none = registration("r", None, 1).expect("an empty body registers");
        assert!(none.settings.is_empty() && none.repo_type == "fs");
        assert!(registration("r", Some(b""), 1).is_ok());
    }

    #[test]
    fn a_registration_the_shell_refuses_is_refused() {
        let kind = |body: &str| put("r", body).unwrap_err();
        assert_eq!(kind(r#"{"type":"s3"}"#).kind, "unsupported_repository_type");
        assert_eq!(
            kind(r#"{"crypto_settings":{"a":1}}"#).kind,
            "unsupported_repository_setting"
        );
        assert_eq!(
            kind(r#"{"type":"s3","crypto_settings":{}}"#).kind,
            "unsupported_repository_type"
        );
        assert_eq!(kind("[]").kind, "malformed_body");
        assert_eq!(
            kind(r#"{"settings":{"a":[1]}}"#).kind,
            "illegal_argument_exception"
        );
        assert_eq!(
            kind(r#"{"settings":{"a":null}}"#).kind,
            "illegal_argument_exception"
        );
        assert_eq!(kind(r#"{"settings":3}"#).kind, "illegal_argument_exception");
        assert_eq!(kind(r#"{"type":3}"#).kind, "illegal_argument_exception");
        assert!(
            put(
                "r",
                r#"{"type":"fs","crypto_settings":null,"settings":null,"verify":false}"#
            )
            .is_ok()
        );
    }

    #[test]
    fn a_later_setting_replaces_an_earlier_one_of_the_same_name() {
        let r = put("r", r#"{"settings":{"a.b":"1","a":{"b":"2"}}}"#).expect("reads");
        assert_eq!(r.settings, [("a.b".to_owned(), "2".to_owned())]);
    }

    #[test]
    fn a_delete_takes_concrete_names_and_validates_all_first() {
        assert_eq!(deletion("a,b").unwrap(), ["a", "b"]);
        assert_eq!(
            deletion("a,_all").unwrap_err().kind,
            "invalid_snapshot_name_exception"
        );
        assert_eq!(deletion("a,_x").unwrap_err().status, 400);
        assert_eq!(
            deletion("a*").unwrap_err().kind,
            "invalid_snapshot_name_exception"
        );
        assert_eq!(deletion("").unwrap_err().status, 400);
    }

    #[test]
    fn the_answers_are_the_shells() {
        assert_eq!(ACKNOWLEDGED, r#"{"acknowledged":true}"#);
        assert_eq!(
            render_verify("n1", "node \"a\""),
            r#"{"nodes":{"n1":{"name":"node \"a\""}}}"#
        );
        assert_eq!(in_use("r").status, 409);
        assert_eq!(missing("r").status, 404);
        assert_eq!(too_many_snapshots_to_check("r").status, 500);
        assert_eq!(unreadable("r").kind, "repository_unreadable");
    }
}
