//! SNS `Publish` and SES v2 `SendEmail`, signed with SigV4 under an account
//! the operator declared (ADR 0081).
//!
//! The credential comes from [`Accounts`] by the `role_arn` the config names,
//! never from the config, and a config naming a role the operator did not
//! declare is refused before anything is signed or sent. The region comes
//! from a topic ARN or a checked region name, so it cannot carry a host. The
//! request goes out through the deliverer, so the egress guard, the slots and
//! the time bound apply, and what the service answers is reduced to a status.
//! These are not S3 requests and are not counted as any.

use std::time::{SystemTime, UNIX_EPOCH};

use oss_blob::s3::sigv4::{
    Scope, amz_date, authorization, canonical_request, sha256_hex, uri_encode,
};
use serde_json::{Value, json};

use super::accounts::{Accounts, AwsAccount};
use super::deliver::{Deliverer, Delivery};

const SNS_VERSION: &str = "2010-03-31";
const SES_PATH: &str = "/v2/email/outbound-emails";

fn form(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        uri_encode(k.as_bytes(), false, &mut out);
        out.push('=');
        uri_encode(v.as_bytes(), false, &mut out);
    }
    out
}

/// Signs and posts `body` to `path` of the service `name` in `region`.
fn signed_post(
    deliverer: &Deliverer,
    account: &AwsAccount,
    name: &str,
    region: &str,
    path: &str,
    content_type: &str,
    body: String,
) -> Delivery {
    if super::config::region(region).is_err() {
        // A region names a host; one that is not a region name is never used.
        return Delivery::failed(400, "no url");
    }
    let base = account
        .endpoint
        .clone()
        // ALLOC: one URL a send.
        .unwrap_or_else(|| format!("https://{name}.{region}.amazonaws.com"));
    let base = base.trim_end_matches('/');
    let Some(authority) = base.split("://").nth(1).filter(|a| !a.is_empty()) else {
        return Delivery::failed(400, "no url");
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let date = amz_date(now);
    let scope = Scope {
        date: date.get(..8).unwrap_or_default(),
        region,
        service: name,
    };
    let payload = sha256_hex(body.as_bytes());
    let mut signed: Vec<(&str, &str)> = vec![
        ("content-type", content_type),
        ("host", authority),
        ("x-amz-date", &date),
    ];
    if let Some(t) = &account.session_token {
        signed.push(("x-amz-security-token", t));
    }
    let mut uri = String::new();
    uri_encode(path.as_bytes(), true, &mut uri);
    let canonical = canonical_request("POST", &uri, "", &signed, &payload);
    let auth = authorization(&account.credentials, &scope, &date, &canonical);
    let mut headers: Vec<(String, String)> = vec![
        ("content-type".to_owned(), content_type.to_owned()),
        ("x-amz-date".to_owned(), date.clone()),
        ("authorization".to_owned(), auth),
    ];
    if let Some(t) = &account.session_token {
        headers.push(("x-amz-security-token".to_owned(), t.clone()));
    }
    // ALLOC: one URL a send.
    let url = format!("{base}{path}");
    deliverer.post(&url, &headers, body.into_bytes())
}

/// Publishes `message` to the topic of an `sns` config block.
pub(super) fn publish_sns(
    deliverer: &Deliverer,
    accounts: &Accounts,
    block: &Value,
    subject: &str,
    message: &str,
) -> Delivery {
    let topic = block["topic_arn"].as_str().unwrap_or_default();
    let region = topic.split(':').nth(3).unwrap_or_default();
    let Some(account) = accounts.aws(block["role_arn"].as_str()) else {
        return Delivery::failed(403, "no account");
    };
    let body = form(&[
        ("Action", "Publish"),
        ("Message", message),
        ("Subject", subject),
        ("TopicArn", topic),
        ("Version", SNS_VERSION),
    ]);
    signed_post(
        deliverer,
        account,
        "sns",
        region,
        "/",
        "application/x-www-form-urlencoded",
        body,
    )
}

/// Sends `text` as a mail from an `ses_account` config block to `to`.
pub(super) fn send_ses(
    deliverer: &Deliverer,
    accounts: &Accounts,
    account_block: &Value,
    to: &[String],
    subject: &str,
    text: &str,
) -> Delivery {
    let region = account_block["region"].as_str().unwrap_or_default();
    let Some(account) = accounts.aws(account_block["role_arn"].as_str()) else {
        return Delivery::failed(403, "no account");
    };
    let body = json!({
        "FromEmailAddress": account_block["from_address"],
        "Destination": {"ToAddresses": to},
        "Content": {"Simple": {
            "Subject": {"Data": subject},
            "Body": {"Text": {"Data": text}},
        }},
    })
    .to_string();
    signed_post(
        deliverer,
        account,
        "ses",
        region,
        SES_PATH,
        "application/json",
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications_route::{EgressPolicy, Limits, system_lookup};

    #[test]
    fn a_region_that_is_not_a_region_name_is_never_used_as_a_host() {
        let accounts =
            Accounts::parse(r#"{"aws":[{"access_key":"AK","secret_key":"SK"}]}"#).expect("parses");
        let d = Deliverer::new(system_lookup(), EgressPolicy::default(), Limits::default());
        let block = json!({"topic_arn": "arn:aws:sns:evil.com/?:1:t"});
        let out = publish_sns(&d, &accounts, &block, "s", "m");
        assert_eq!(out.status_code, 400);
    }
}
