//! The answers of the account API: a user reading what it is and changing
//! its own password.
//!
//! Shapes are the recorded 3.10.0 answers
//! (`fixtures/compat/security-api/security-api.json`).

use oss_doc::mapping::json::{Json, parse, write_str};

use super::{Answer, Config};

fn names<'a>(out: &mut String, items: impl Iterator<Item = &'a str>) {
    out.push('[');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(out, item);
    }
    out.push(']');
}

/// The body `GET /_plugins/_security/api/account` answers for `name`. The
/// static administrator is reserved, as the plugin's own administrator is.
pub fn account(config: &Config, name: &str) -> String {
    let user = config.user(name);
    let backend = user.map(|u| u.backend_roles.as_slice()).unwrap_or_default();
    // ALLOC: one answer a request.
    let mut out = String::from(r#"{"user_name":"#);
    write_str(&mut out, name);
    out.push_str(r#","is_reserved":"#);
    out.push_str(if config.is_admin(name) {
        "true"
    } else {
        "false"
    });
    out.push_str(r#","is_hidden":false,"is_internal_user":true,"user_requested_tenant":null,"backend_roles":"#);
    names(&mut out, backend.iter().map(String::as_str));
    out.push_str(r#","custom_attribute_names":"#);
    // ALLOC: one answer a request, bounded by the stored document.
    let attributes: Vec<String> = user
        .map(|u| {
            u.attributes
                .iter()
                // ALLOC: one answer a request, bounded by the stored document.
                .map(|a| format!("attr.internal.{a}"))
                // ALLOC: one answer a request, bounded by the stored document.
                .collect()
        })
        .unwrap_or_default();
    names(&mut out, attributes.iter().map(String::as_str));
    out.push_str(r#","tenants":{"#);
    for (i, t) in config.tenants_of(name).iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, t);
        out.push_str(":true");
    }
    out.push_str(r#"},"roles":"#);
    let roles = config.roles_of(name);
    names(&mut out, roles.iter().map(String::as_str));
    out.push('}');
    out
}

fn bad_request(message: &str) -> Answer {
    // ALLOC: one answer a request.
    let mut text = String::new();
    write_str(&mut text, message);
    Answer {
        status: 400,
        // ALLOC: one answer a request, bounded by the stored document.
        body: format!(r#"{{"status":"BAD_REQUEST","message":{text}}}"#),
    }
}

/// The `current_password` and `password` of a password change.
///
/// # Errors
///
/// The `400` of a body that is not one JSON object holding both as strings.
pub fn password_change(body: &[u8]) -> Result<(String, String), Answer> {
    let wrong = || bad_request("Please provide 'current_password' and 'password'.");
    let Ok(Json::Obj(members)) = parse(body) else {
        return Err(wrong());
    };
    let text = |key: &str| {
        members.iter().find_map(|(k, v)| match v {
            Json::Str(s) if k == key => Some(s.clone()),
            _ => None,
        })
    };
    match (text("current_password"), text("password")) {
        (Some(current), Some(new)) => Ok((current, new)),
        _ => Err(wrong()),
    }
}

/// The `400` for a current password that is not the user's.
pub fn wrong_current() -> Answer {
    bad_request("Could not validate your current password.")
}

/// The `403` for the static administrator, whose password is not stored.
pub fn account_reserved(name: &str) -> Answer {
    // ALLOC: one answer a request.
    let mut text = String::new();
    write_str(&mut text, &format!("Resource '{name}' is reserved."));
    Answer {
        status: 403,
        // ALLOC: one answer a request, bounded by the stored document.
        body: format!(r#"{{"status":"FORBIDDEN","message":{text}}}"#),
    }
}
