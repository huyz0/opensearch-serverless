//! `/feature/test/{id}`: send the plugin's test message through a config.

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::NotificationsService;
use super::config::is_channel;
use super::deliver::Delivery;
use crate::route_util::refuse;

/// The message the test route sends.
const TEST_TEXT: &str = "Test message from the notifications plugin.";

/// The words of a failed delivery in the plugin's own phrasing: the node's
/// fixed word, never an address or anything the target answered with.
fn phrase(word: &str) -> String {
    let reason = if let Some(code) = word.strip_prefix("status ") {
        code.parse::<u16>()
            .ok()
            .and_then(|c| ureq::http::StatusCode::from_u16(c).ok())
            .and_then(|c| c.canonical_reason())
            .unwrap_or("Failed")
    } else {
        match word {
            "blocked" => "Forbidden",
            "timeout" => "Timeout",
            "rejected" => "Too Many Requests",
            "too large" => "Payload Too Large",
            "no url" => "Bad Request",
            _ => "Connection failed",
        }
    };
    // ALLOC: error path; one short message.
    format!("Failed to send webhook message Failed: {reason}")
}

/// One entry of a plugin's `event_status_list` / `status_list`.
fn entry(id: &str, ty: &str, name: &Value, status: u16, text: &str) -> Value {
    json!({
        "config_id": id,
        "config_type": ty,
        "config_name": name,
        "email_recipient_status": [],
        "delivery_status": {"status_code": status.to_string(), "status_text": text},
    })
}

/// A failed test as the plugin answers: the status of the delivery, and the
/// entry in a message that is itself JSON.
fn failed(id: &str, ty: &str, name: &Value, status: u16, text: &str) -> RestError {
    // The plugin's own key order, which a JSON map does not keep.
    let q = |v: &Value| v.to_string();
    // ALLOC: error path; one message.
    let one = format!(
        "{{\"config_id\":{},\"config_type\":{},\"config_name\":{},\"email_recipient_status\":[],\"delivery_status\":{{\"status_code\":\"{status}\",\"status_text\":{}}}}}",
        q(&json!(id)),
        q(&json!(ty)),
        q(name),
        q(&json!(text)),
    );
    refuse(
        status,
        "status_exception",
        // ALLOC: error path; one message.
        format!("{{\"event_status_list\": [{one}]}}"),
    )
}

impl NotificationsService {
    /// Sends the test message through the config `id` and answers the
    /// plugin's `status_list`. A delivery that failed is the plugin's error
    /// with its status; a config the node cannot send to (email, SNS, SES in
    /// this build) says so.
    pub(crate) fn test_send(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let Some(deliverer) = &self.deliverer else {
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
            return Err(failed(
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
            return Err(failed(&id, ty, name, 423, "The channel is muted"));
        }
        if !is_channel(ty) || matches!(ty, "email" | "sns") {
            return Err(failed(
                &id,
                ty,
                name,
                503,
                "Sending to this channel type is not served by this node",
            ));
        }
        let outcome: Delivery = deliverer.deliver(config, TEST_TEXT);
        if !(200..300).contains(&outcome.status_code) {
            return Err(failed(&id, ty, name, 500, &phrase(&outcome.status_text)));
        }
        let body = json!({
            "event_source": {
                "title": format!("Test Message Title-{id}"),
                "reference_id": id,
                "severity": "info",
                "tags": [],
            },
            "status_list": [entry(&id, ty, name, 200, &outcome.status_text)],
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
