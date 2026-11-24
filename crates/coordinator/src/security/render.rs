//! The answers of the management API: acknowledgements and documents.

use oss_doc::mapping::json::{Json, write_str};

use super::{Answer, Kind};

fn message(status: u16, label: &str, name: &str, tail: &str) -> Answer {
    // ALLOC: one message a request.
    let mut text = String::new();
    write_str(&mut text, &format!("'{name}' {tail}."));
    Answer {
        status,
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        body: format!(r#"{{"status":"{label}","message":{text}}}"#),
    }
}

/// The answer to a put: 201 when the document is new, 200 when replaced.
pub fn saved(name: &str, created: bool) -> Answer {
    if created {
        message(201, "CREATED", name, "created")
    } else {
        message(200, "OK", name, "updated")
    }
}

/// The answer to a delete.
pub fn deleted(name: &str) -> Answer {
    message(200, "OK", name, "deleted")
}

/// The answer to a read or delete of a document that is not there.
pub fn missing(kind: Kind, name: &str) -> Answer {
    // ALLOC: one message a request.
    let mut text = String::new();
    write_str(&mut text, &format!("{} '{name}' not found.", kind.noun()));
    Answer {
        status: 404,
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        body: format!(r#"{{"status":"NOT_FOUND","message":{text}}}"#),
    }
}

fn member<'a>(doc: &'a Json, key: &str) -> Option<&'a Json> {
    doc.get(key)
}

fn or(doc: &Json, key: &str, default: Json) -> (String, Json) {
    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
    (key.to_owned(), member(doc, key).cloned().unwrap_or(default))
}

fn when_present(doc: &Json, key: &str, out: &mut Vec<(String, Json)>) {
    if let Some(v) = member(doc, key) {
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        out.push((key.to_owned(), v.clone()));
    }
}

fn empty() -> Json {
    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
    Json::Arr(Vec::new())
}

/// `hidden` and `reserved`, and `static` for every kind but a mapping (the
/// recorded role mapping answer has none).
fn flags(kind: Kind, out: &mut Vec<(String, Json)>) {
    for key in ["hidden", "reserved", "static"] {
        if key != "static" || kind != Kind::RolesMapping {
            // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
            out.push((key.to_owned(), Json::Bool(false)));
        }
    }
}

fn entries(doc: &Json, key: &str, defaults: &[&str]) -> (String, Json) {
    let list = match member(doc, key) {
        Some(Json::Arr(items)) => items
            .iter()
            .map(|item| {
                let Json::Obj(have) = item else {
                    return item.clone();
                };
                let mut out = have.clone();
                for d in defaults {
                    if !have.iter().any(|(k, _)| k == d) {
                        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
                        out.push(((*d).to_owned(), empty()));
                    }
                }
                Json::Obj(out)
            })
            // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
            .collect(),
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        _ => Vec::new(),
    };
    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
    (key.to_owned(), Json::Arr(list))
}

/// A stored document as the plugin answers a read: `{"<name>": {...}}`,
/// with the defaults the plugin adds. A user's `hash` is never returned.
pub fn render(kind: Kind, name: &str, doc: &Json) -> String {
    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
    let mut out: Vec<(String, Json)> = Vec::new();
    match kind {
        Kind::Roles => {
            out.push(or(doc, "cluster_permissions", empty()));
            when_present(doc, "description", &mut out);
            out.push(entries(doc, "index_permissions", &["fls", "masked_fields"]));
            out.push(entries(doc, "tenant_permissions", &[]));
        }
        Kind::RolesMapping => {
            for key in ["and_backend_roles", "backend_roles", "hosts", "users"] {
                out.push(or(doc, key, empty()));
            }
            when_present(doc, "description", &mut out);
        }
        Kind::InternalUsers => {
            // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
            out.push(or(doc, "attributes", Json::Obj(Vec::new())));
            out.push(or(doc, "backend_roles", empty()));
            when_present(doc, "description", &mut out);
            // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
            out.push(("hash".to_owned(), Json::Str(String::new())));
            out.push(or(doc, "opendistro_security_roles", empty()));
        }
        // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
        Kind::Tenants => out.push(or(doc, "description", Json::Str(String::new()))),
        Kind::ActionGroups => {
            out.push(or(doc, "allowed_actions", empty()));
            when_present(doc, "description", &mut out);
            when_present(doc, "type", &mut out);
        }
    }
    flags(kind, &mut out);
    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
    let mut text = String::new();
    write_str(&mut text, name);
    // ALLOC: one answer a request, bounded by the 1 MiB body and its key count.
    format!("{{{text}:{}}}", Json::Obj(out).to_json())
}
