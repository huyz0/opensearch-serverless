//! A channel config as a client puts it: checked at put against what this
//! node can send, so a bad shape is a 400 naming it and never a failure at
//! send. Answers use the plugin's own statuses and words (the stock 3.10.0
//! recording): an unknown key is dropped, a type without its block is a 400,
//! an unknown type a 406.

use oss_rest::RestError;
use serde_json::{Map, Value, json};

use crate::route_util::refuse;

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
        address(text(o, "recipient", "recipient")?, "recipient")?;
    }
    Ok(())
}

fn status(code: u16, reason: impl Into<String>) -> RestError {
    refuse(code, "status_exception", reason)
}

/// The ids a config names: the account an email sends through and the
/// groups it sends to. A put checks that each exists.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Refs {
    pub accounts: Vec<String>,
    pub groups: Vec<String>,
}

fn arg(reason: String) -> RestError {
    super::bad(reason)
}

/// The block of `ty`, as the plugin stores it: the keys it knows, with its
/// defaults, and what an email names.
fn block(ty: &str, b: &Map<String, Value>) -> Result<(Value, Refs), RestError> {
    let mut refs = Refs::default();
    let out = match ty {
        "slack" => {
            let u = text(b, "url", ty).map_err(arg)?;
            if !u.contains("hooks.slack.com/services/")
                && !u.contains("hooks.gov-slack.com/services/")
            {
                return Err(status(
                    400,
                    "Wrong Slack url. Should contain \"hooks.slack.com/services/\" or \"hooks.gov-slack.com/services/\"",
                ));
            }
            url(u, ty).map_err(arg)?;
            json!({"url": u})
        }
        "chime" => {
            let u = text(b, "url", ty).map_err(arg)?;
            if !u.contains("hooks.chime.aws/incomingwebhooks/") || !u.contains("?token=") {
                return Err(status(
                    400,
                    "Wrong Chime url. Should contain \"hooks.chime.aws/incomingwebhooks/\" and \"?token=\"",
                ));
            }
            url(u, ty).map_err(arg)?;
            json!({"url": u})
        }
        "microsoft_teams" => {
            let u = text(b, "url", ty).map_err(arg)?;
            url(u, ty).map_err(arg)?;
            json!({"url": u})
        }
        "webhook" => {
            let u = text(b, "url", ty).map_err(arg)?;
            if url(u, ty).is_err() {
                return Err(arg("Invalid URL or unsupported".to_owned()));
            }
            let h = match b.get("header_params") {
                None | Some(Value::Null) => json!({}),
                Some(h) => {
                    headers(h).map_err(arg)?;
                    h.clone()
                }
            };
            let method = match b.get("method") {
                None | Some(Value::Null) => "POST",
                Some(Value::String(m)) if matches!(m.as_str(), "POST" | "PUT" | "PATCH") => {
                    m.as_str()
                }
                _ => return Err(arg("[webhook] [method] is POST, PUT or PATCH".to_owned())),
            };
            json!({"url": u, "header_params": h, "method": method})
        }
        "email" => {
            let account = text(b, "email_account_id", ty).map_err(arg)?;
            refs.accounts.push(account.to_owned());
            let list = match b.get("recipient_list") {
                None | Some(Value::Null) => json!([]),
                Some(l) => {
                    recipients(l, ty).map_err(arg)?;
                    l.clone()
                }
            };
            let groups = match b.get("email_group_id_list") {
                None | Some(Value::Null) => json!([]),
                Some(Value::Array(ids))
                    if ids.len() <= MAX_RECIPIENTS
                        && ids
                            .iter()
                            .all(|i| i.as_str().is_some_and(|s| !s.is_empty())) =>
                {
                    refs.groups
                        .extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
                    Value::Array(ids.clone())
                }
                Some(_) => {
                    return Err(arg(
                        "[email] [email_group_id_list] must be an array of ids".to_owned()
                    ));
                }
            };
            json!({"email_account_id": account, "recipient_list": list, "email_group_id_list": groups})
        }
        "sns" => {
            let topic = b
                .get("topic_arn")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !sns_topic(topic) {
                return Err(arg(format!("Invalid AWS SNS topic ARN: {topic}")));
            }
            arn(b, "role_arn", ty, false).map_err(arg)?;
            let mut o = Map::new();
            o.insert("topic_arn".into(), Value::String(topic.to_owned()));
            if let Some(r) = b.get("role_arn") {
                o.insert("role_arn".into(), r.clone());
            }
            Value::Object(o)
        }
        "ses_account" => {
            region(text(b, "region", ty).map_err(arg)?).map_err(arg)?;
            address(text(b, "from_address", ty).map_err(arg)?, "from_address").map_err(arg)?;
            arn(b, "role_arn", ty, false).map_err(arg)?;
            let mut o = Map::new();
            for k in ["region", "role_arn", "from_address"] {
                if let Some(v) = b.get(k) {
                    o.insert(k.into(), v.clone());
                }
            }
            Value::Object(o)
        }
        "smtp_account" => {
            let host = text(b, "host", ty).map_err(arg)?;
            let port = b
                .get("port")
                .and_then(Value::as_u64)
                .filter(|p| (1..=65535).contains(p))
                .ok_or_else(|| arg("[smtp_account] needs a [port] from 1 to 65535".to_owned()))?;
            address(text(b, "from_address", ty).map_err(arg)?, "from_address").map_err(arg)?;
            let method = b
                .get("method")
                .and_then(Value::as_str)
                .filter(|m| matches!(*m, "ssl" | "start_tls"))
                .unwrap_or("none");
            json!({"host": host, "port": port, "method": method,
                   "from_address": b["from_address"]})
        }
        _ => {
            let list = b.get("recipient_list").unwrap_or(&Value::Null);
            recipients(list, ty).map_err(arg)?;
            json!({"recipient_list": list})
        }
    };
    Ok((out, refs))
}

/// `arn:aws:sns:<region>:<account>:<topic>` with a region and account of the
/// shapes AWS gives.
fn sns_topic(s: &str) -> bool {
    let p: Vec<&str> = s.split(':').collect();
    p.len() == 6
        && p[0] == "arn"
        && p[1].starts_with("aws")
        && p[2] == "sns"
        && region(p[3]).is_ok()
        && !p[4].is_empty()
        && p[4].bytes().all(|b| b.is_ascii_digit())
        && !p[5].is_empty()
        && p[5]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// The config of a request, checked and rebuilt from the keys the plugin
/// knows, with its defaults (`description` empty, `is_enabled` true), and
/// what it refers to.
pub(super) fn checked(
    v: &Value,
    allowed: &[&str],
) -> Result<(Map<String, Value>, Refs), RestError> {
    let m = v
        .as_object()
        .ok_or_else(|| arg("[config] must be an object".to_owned()))?;
    let ty = text(m, "config_type", "config").map_err(arg)?;
    if !TYPES.contains(&ty) || !allowed.contains(&ty) {
        return Err(status(
            406,
            "NotificationConfig with type NONE is not acceptable",
        ));
    }
    let matching = m.get(ty).and_then(Value::as_object);
    let foreign = TYPES.iter().any(|t| *t != ty && m.contains_key(*t));
    let Some(b) = matching.filter(|_| !foreign) else {
        return Err(arg(format!("ConfigType: {ty} and data doesn't match")));
    };
    let name = text(m, "name", "config").map_err(arg)?;
    let mut out = Map::new();
    out.insert("name".into(), Value::String(name.to_owned()));
    let description = match m.get("description") {
        None | Some(Value::Null) => "",
        Some(Value::String(d)) => d.as_str(),
        Some(_) => return Err(arg("[description] must be a string".to_owned())),
    };
    out.insert("description".into(), Value::String(description.to_owned()));
    out.insert("config_type".into(), Value::String(ty.to_owned()));
    let enabled = match m.get("is_enabled") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(arg("[is_enabled] must be a boolean".to_owned())),
    };
    out.insert("is_enabled".into(), Value::Bool(enabled));
    let (shown, refs) = block(ty, b)?;
    out.insert(ty.to_owned(), shown);
    Ok((out, refs))
}
