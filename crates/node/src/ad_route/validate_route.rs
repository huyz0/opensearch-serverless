//! `POST /detectors/_validate[/detector]`: whether a detector body would be
//! accepted, answered as the plugin does: `{}` when it would, else the field
//! and the reason.

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::{AdService, bad, fresh_id, now_millis};
use crate::route_util::read_body;

/// The field a refusal is about: the name in its leading `[brackets]`, the
/// features for a message about them, else the detector as a whole.
fn field_of(reason: &str) -> &str {
    if let Some((name, _)) = reason.strip_prefix('[').and_then(|r| r.split_once(']')) {
        return name;
    }
    if reason.contains("feature") {
        return "feature_attributes";
    }
    "detector"
}

impl AdService {
    /// Checks the body as a create would, and writes nothing.
    pub(crate) fn validate(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        if let Some(kind) = req.path_param("type").filter(|k| k != "detector") {
            return Err(bad(format!(
                "validation of [{kind}] is not served: only [detector] is"
            )));
        }
        let max = DocKind::AdDetectors.max_doc_bytes();
        let raw = read_body(req, max, "the body is over the limit for a detector")?;
        let body: Value =
            serde_json::from_slice(&raw).map_err(|e| bad(format!("invalid body: {e}")))?;
        let out = match super::detector::checked(&body, now_millis(), &mut fresh_id) {
            Ok(_) => json!({}),
            Err(reason) => json!({"detector": {field_of(&reason): {"message": reason}}}),
        };
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::field_of;

    #[test]
    fn a_refusal_names_the_field_it_is_about() {
        assert_eq!(field_of("[name] is required"), "name");
        assert_eq!(
            field_of("[shingle_size] must be a whole number"),
            "shingle_size"
        );
        assert_eq!(
            field_of("feature names must be unique"),
            "feature_attributes"
        );
        assert_eq!(
            field_of("a detector must have between 1 and 5 features, found 0"),
            "feature_attributes"
        );
        assert_eq!(field_of("something else"), "detector");
    }
}
