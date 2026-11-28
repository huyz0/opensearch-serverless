//! A channel config as a client puts it: checked at put against what this
//! node can send, so a bad shape is a 400 naming it and never a failure at
//! send.

use serde_json::{Map, Value};

/// Every `config_type` of the plugin.
pub(super) const TYPES: [&str; 9] = [
    "slack",
    "chime",
    "microsoft_teams",
    "webhook",
    "email",
    "sns",
    "ses_account",
    "smtp_account",
    "email_group",
];

/// The types a client sends to, as the `channels` route lists them.
const CHANNEL_TYPES: [&str; 6] = [
    "slack",
    "chime",
    "microsoft_teams",
    "webhook",
    "email",
    "sns",
];

/// Header names a webhook may not set: the node frames its own request.
const FORBIDDEN_HEADERS: [&str; 4] = ["host", "content-length", "transfer-encoding", "connection"];

const MAX_NAME_CHARS: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_RECIPIENTS: usize = 100;
const MAX_HEADERS: usize = 32;

/// Whether `config_type` is one a client sends to.
pub(super) fn is_channel(config_type: &str) -> bool {
    CHANNEL_TYPES.contains(&config_type)
}

type Checked = Result<(), String>;

fn text<'a>(m: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a str, String> {
    match m.get(key).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => Err(format!("[{what}] needs a non-empty string [{key}]")),
    }
}

fn only(m: &Map<String, Value>, allowed: &[&str], what: &str) -> Checked {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unknown key [{k}] in [{what}]")),
        None => Ok(()),
    }
}

/// An `http` or `https` URL with a host and no userinfo.
fn url(s: &str, what: &str) -> Checked {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .ok_or_else(|| format!("[{what}] [url] must be an http or https URL"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let bad = authority.is_empty()
        || authority.contains('@')
        || s.chars().any(|c| c.is_control() || c.is_whitespace());
    if bad {
        return Err(format!(
            "[{what}] [url] needs a host, no user information and no spaces"
        ));
    }
    Ok(())
}

fn address(s: &str, what: &str) -> Checked {
    let mut parts = s.split('@');
    let ok = matches!((parts.next(), parts.next(), parts.next()),
        (Some(a), Some(b), None) if !a.is_empty() && !b.is_empty())
        && !s.chars().any(|c| c.is_control() || c.is_whitespace());
    if ok {
        Ok(())
    } else {
        Err(format!("[{what}] [{s}] is not an email address"))
    }
}

/// `us-east-1` and its kin: lower-case words and one digit, so a region can
/// not carry a host.
fn region(s: &str) -> Checked {
    let parts: Vec<&str> = s.split('-').collect();
    let lower = |p: &&str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase());
    let ok = match parts.split_first().zip(parts.split_last()) {
        Some(((first, _), (last, _))) if parts.len() >= 3 => {
            first.len() == 2
                && lower(first)
                && last.len() == 1
                && last.bytes().all(|b| b.is_ascii_digit())
                && parts
                    .iter()
                    .skip(1)
                    .take(parts.len().saturating_sub(2))
                    .all(lower)
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("[region] [{s}] is not an AWS region name"))
    }
}

fn arn(m: &Map<String, Value>, key: &str, what: &str, required: bool) -> Checked {
    match m.get(key) {
        None if !required => Ok(()),
        Some(Value::String(s)) if s.starts_with("arn:") && !s.chars().any(char::is_whitespace) => {
            Ok(())
        }
        _ => Err(format!("[{what}] needs [{key}] as an ARN")),
    }
}

fn headers(h: &Value) -> Checked {
    let m = h
        .as_object()
        .ok_or("[header_params] must be an object of strings")?;
    if m.len() > MAX_HEADERS {
        return Err(format!(
            "[header_params] holds at most {MAX_HEADERS} headers"
        ));
    }
    for (name, value) in m {
        let token = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
        if !token || FORBIDDEN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(format!("[header_params] header [{name}] is not allowed"));
        }
        match value.as_str() {
            Some(v) if !v.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) => {}
            _ => {
                return Err(format!(
                    "[header_params] header [{name}] needs a plain string"
                ));
            }
        }
    }
    Ok(())
}

fn recipients(list: &Value, what: &str) -> Checked {
    let items = list
        .as_array()
        .ok_or_else(|| format!("[{what}] [recipient_list] must be an array"))?;
    if items.len() > MAX_RECIPIENTS {
        return Err(format!(
            "[{what}] holds at most {MAX_RECIPIENTS} recipients"
        ));
    }
    for item in items {
        let o = item
            .as_object()
            .ok_or_else(|| format!("[{what}] a recipient is an object with [recipient]"))?;
        only(o, &["recipient"], "recipient")?;
        address(text(o, "recipient", "recipient")?, "recipient")?;
    }
    Ok(())
}

fn block(ty: &str, b: &Map<String, Value>) -> Checked {
    match ty {
        "slack" | "chime" | "microsoft_teams" => {
            only(b, &["url"], ty)?;
            url(text(b, "url", ty)?, ty)
        }
        "webhook" => {
            only(b, &["url", "header_params", "method"], ty)?;
            url(text(b, "url", ty)?, ty)?;
            if let Some(h) = b.get("header_params") {
                headers(h)?;
            }
            match b.get("method").map(Value::as_str) {
                None | Some(Some("POST" | "PUT" | "PATCH")) => Ok(()),
                _ => Err("[webhook] [method] is POST, PUT or PATCH".to_owned()),
            }
        }
        "email" => {
            only(
                b,
                &["email_account_id", "recipient_list", "email_group_id_list"],
                ty,
            )?;
            text(b, "email_account_id", ty)?;
            if let Some(list) = b.get("recipient_list") {
                recipients(list, ty)?;
            }
            match b.get("email_group_id_list") {
                None => Ok(()),
                Some(Value::Array(ids))
                    if ids.len() <= MAX_RECIPIENTS
                        && ids
                            .iter()
                            .all(|i| i.as_str().is_some_and(|s| !s.is_empty())) =>
                {
                    Ok(())
                }
                Some(_) => Err("[email] [email_group_id_list] must be an array of ids".to_owned()),
            }
        }
        "sns" => {
            only(b, &["topic_arn", "role_arn"], ty)?;
            arn(b, "topic_arn", ty, true)?;
            arn(b, "role_arn", ty, false)
        }
        "ses_account" => {
            only(b, &["region", "role_arn", "from_address"], ty)?;
            region(text(b, "region", ty)?)?;
            address(text(b, "from_address", ty)?, "from_address")?;
            arn(b, "role_arn", ty, false)
        }
        "smtp_account" => {
            only(b, &["account_id", "from_address"], ty)?;
            text(b, "account_id", ty)?;
            address(text(b, "from_address", ty)?, "from_address")
        }
        _ => {
            only(b, &["recipient_list"], ty)?;
            recipients(b.get("recipient_list").unwrap_or(&Value::Null), ty)
        }
    }
}

/// The config of a request, checked, with its defaults filled in
/// (`description` empty, `is_enabled` true).
pub(super) fn checked(v: &Value, allowed: &[&str]) -> Result<Map<String, Value>, String> {
    let m = v.as_object().ok_or("[config] must be an object")?;
    let ty = text(m, "config_type", "config")?;
    if !TYPES.contains(&ty) || !allowed.contains(&ty) {
        return Err(format!("[config_type] [{ty}] is not a supported type"));
    }
    only(
        m,
        &["name", "description", "config_type", "is_enabled", ty],
        "config",
    )?;
    let name = text(m, "name", "config")?;
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("[name] is at most {MAX_NAME_CHARS} characters"));
    }
    let mut out = m.clone();
    match m.get("description") {
        None => {
            out.insert("description".into(), Value::String(String::new()));
        }
        Some(Value::String(d)) if d.chars().count() <= MAX_DESCRIPTION_CHARS => {}
        Some(_) => {
            return Err(format!(
                "[description] is a string of at most {MAX_DESCRIPTION_CHARS} characters"
            ));
        }
    }
    match m.get("is_enabled") {
        None => {
            out.insert("is_enabled".into(), Value::Bool(true));
        }
        Some(Value::Bool(_)) => {}
        Some(_) => return Err("[is_enabled] must be a boolean".to_owned()),
    }
    let b = m
        .get(ty)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("[config] needs a [{ty}] object"))?;
    block(ty, b)?;
    Ok(out)
}
