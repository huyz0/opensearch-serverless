//! `/feature/test/{id}`: send the plugin's test message through a config.

use std::cell::RefCell;
use std::sync::Arc;

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::NotificationsService;
use super::deliver::Delivery;
use super::send::Outcome;
use crate::route_util::refuse;

/// The message the test route sends.
const TEST_TEXT: &str = "Test message from the notifications plugin.";

/// The reason a failed delivery gives, as a fixed word and never what the
/// target answered with.
fn reason(word: &str) -> &'static str {
    if let Some(code) = word.strip_prefix("status ") {
        return code
            .parse::<u16>()
            .ok()
            .and_then(|c| ureq::http::StatusCode::from_u16(c).ok())
            .and_then(|c| c.canonical_reason())
            .unwrap_or("Failed");
    }
    match word {
        "blocked" => "Forbidden",
        "timeout" => "Timeout",
        "rejected" => "Too Many Requests",
        "too large" => "Payload Too Large",
        "no url" => "Bad Request",
        "no account" => "No account",
        "no recipients" => "No recipients",
        "bad address" => "Bad Request",
        "not served" => "Not served",
        _ => "Connection failed",
    }
}

/// The plugin's status code and text for a delivery of a config of `ty`.
fn plugin_status(ty: &str, d: &Delivery) -> (u16, String) {
    if (200..300).contains(&d.status_code) {
        return (200, d.status_text.clone());
    }
    if ty == "email" {
        // ALLOC: error path; one short message.
        return (
            503,
            format!("sendEmail Error, status:{}", reason(&d.status_text)),
        );
    }
    // ALLOC: error path; one short message.
    (
        500,
        format!(
            "Failed to send {ty} message Failed: {}",
            reason(&d.status_text)
        ),
    )
}

fn status_json(code: u16, text: &str) -> Value {
    json!({"status_code": code.to_string(), "status_text": text})
}

/// One entry of the plugin's `event_status_list`, keys in its order, which a
/// JSON map does not keep.
fn entry_text(
    id: &str,
    ty: &str,
    name: &Value,
    recipients: &str,
    delivery: Option<&Value>,
) -> String {
    let q = |v: &Value| v.to_string();
    let tail = delivery.map_or_else(String::new, |d| format!(",\"delivery_status\":{}", q(d)));
    // ALLOC: error path; one message.
    format!(
        "{{\"config_id\":{},\"config_type\":{},\"config_name\":{},\"email_recipient_status\":[{recipients}]{tail}}}",
        q(&json!(id)),
        q(&json!(ty)),
        q(name),
    )
}

/// A failed test as the plugin answers: the status of the delivery, and the
/// entry in a message that is itself JSON.
fn failed(entry: &str, status: u16) -> RestError {
    refuse(
        status,
        "status_exception",
        // ALLOC: error path; one message.
        format!("{{\"event_status_list\": [{entry}]}}"),
    )
}

/// An answer for a config that cannot be sent to at all.
fn refused(id: &str, ty: &str, name: &Value, status: u16, text: &str) -> RestError {
    let d = status_json(status, text);
    failed(&entry_text(id, ty, name, "", Some(&d)), status)
}

impl NotificationsService {
    /// Sends the test message through the config `id` and answers the
    /// plugin's `status_list`. A delivery that failed is the plugin's error
    /// with its status.
    pub(crate) fn test_send(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let Some(senders) = &self.senders else {
            return Err(refuse(
                501,
                "status_exception",
                "sending notifications is not configured",
            ));
        };
        let id = Self::id_of(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req, DocKind::NotificationConfigs, 1)?;
        let Some(stored) = self.stored(&id, &mut marker, &mut doc)? else {
            return Err(refused(
                &id,
                "none",
                &json!("invalid-config"),
                404,
                &format!("Channel {id} not found"),
            ));
        };
        let config = &stored["config"];
        let ty = config["config_type"].as_str().unwrap_or_default();
        let name = &config["name"];
        if config["is_enabled"].as_bool() != Some(true) {
            return Err(refused(&id, ty, name, 423, "The channel is muted"));
        }
        if !super::config::is_channel(ty) {
            return Err(refused(
                &id,
                ty,
                name,
                503,
                "Sending to this channel type is not served by this node",
            ));
        }
        // The account and the groups an email names, read through the same buffers.
        let buffers = RefCell::new((marker, doc));
        let resolve = |other: &str| -> Option<Arc<Value>> {
            let mut b = buffers.borrow_mut();
            let (m, d) = &mut *b;
            self.stored(other, m, d)
                .ok()
                .flatten()
                .map(|s| Arc::new(s["config"].clone()))
        };
        let title = format!("Test Message Title-{id}");
        let outcome: Outcome = senders.send(config, &title, TEST_TEXT, &resolve);
        let shown = |d: &Delivery| {
            let (code, text) = plugin_status(ty, d);
            (code, status_json(code, &text))
        };
        let recipients: Vec<(String, u16, Value)> = outcome
            .recipients
            .iter()
            .map(|(to, d)| {
                let (code, v) = shown(d);
                (to.clone(), code, v)
            })
            .collect();
        let channel = outcome.delivery.as_ref().map(shown);
        let worst = channel
            .iter()
            .map(|(c, _)| *c)
            .chain(recipients.iter().map(|(_, c, _)| *c))
            .find(|c| *c != 200);
        if let Some(code) = worst {
            let list = recipients
                .iter()
                .map(|(to, _, v)| {
                    format!("{{\"recipient\":{},\"delivery_status\":{v}}}", json!(to))
                })
                .collect::<Vec<_>>()
                .join(",");
            let delivery = channel.as_ref().map(|(_, v)| v);
            return Err(failed(&entry_text(&id, ty, name, &list, delivery), code));
        }
        let mut entry = json!({
            "config_id": id,
            "config_type": ty,
            "config_name": name,
            "email_recipient_status": recipients.iter()
                .map(|(to, _, v)| json!({"recipient": to, "delivery_status": v}))
                .collect::<Vec<_>>(),
        });
        if let Some((_, v)) = &channel {
            entry["delivery_status"] = v.clone();
        }
        let body = json!({
            "event_source": {
                "title": title,
                "reference_id": id,
                "severity": "info",
                "tags": [],
            },
            "status_list": [entry],
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
