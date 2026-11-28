//! `POST /feature/test/{id}`: send the plugin's test message through a config.

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::json;

use super::config::is_channel;
use super::deliver::Delivery;
use super::{NotificationsService, not_found};
use crate::route_util::refuse;

/// The message the test route sends.
const TEST_TEXT: &str = "Test message from the notifications plugin.";

impl NotificationsService {
    /// Sends the test message through the config `id` and answers the
    /// plugin's `status_list`. A delivery that failed is a status, not an
    /// error; a config the node cannot send to (email, SNS, SES in this
    /// build) says so in its status.
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
        let stored = self
            .stored(&id, &mut marker, &mut doc)?
            .ok_or_else(|| not_found(&id))?;
        let config = &stored["config"];
        let ty = config["config_type"].as_str().unwrap_or_default();
        let outcome = if config["is_enabled"].as_bool() != Some(true) {
            Delivery {
                status_code: 403,
                status_text: "disabled".to_owned(),
            }
        } else if is_channel(ty) && !matches!(ty, "email" | "sns") {
            deliverer.deliver(config, TEST_TEXT)
        } else {
            Delivery {
                status_code: 501,
                status_text: "not served".to_owned(),
            }
        };
        let body = json!({
            "event_source": {
                "title": "Test Message Title",
                "reference_id": id,
                "severity": "info",
                "tags": [],
            },
            "status_list": [{
                "config_id": id,
                "config_type": ty,
                "config_name": config["name"],
                "delivery_status": outcome.shown(),
            }],
        });
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
