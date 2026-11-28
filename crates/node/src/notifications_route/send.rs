//! One message to one config, whatever its type: the HTTP channels through
//! the deliverer, SNS and SES through SigV4 under an operator's account. The
//! test route and the in-process notifier both send through here.

use std::sync::Arc;

use serde_json::Value;

use super::accounts::Accounts;
use super::aws;
use super::deliver::{Deliverer, Delivery};
use super::smtp;

/// The most recipients a message is addressed to, as the stock plugin bounds them.
const MAX_RECIPIENTS: usize = 100;
/// The most addresses one SES request carries.
const SES_BATCH: usize = 50;

/// What a send came to: one status for a channel, one a recipient for an email.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The status of a channel send; none for an email.
    pub delivery: Option<Delivery>,
    /// Each recipient of an email and its status.
    pub recipients: Vec<(String, Delivery)>,
}

impl Outcome {
    fn one(d: Delivery) -> Self {
        Self {
            delivery: Some(d),
            recipients: Vec::new(),
        }
    }

    /// The first status that is not a success, none when every send worked.
    #[must_use]
    pub fn failure(&self) -> Option<&Delivery> {
        self.delivery
            .iter()
            .chain(self.recipients.iter().map(|(_, d)| d))
            .find(|d| !(200..300).contains(&d.status_code))
    }

    /// The status a caller that wants one reads: the first failure, or the
    /// first success.
    #[must_use]
    pub fn summary(&self) -> Delivery {
        self.failure()
            .or(self.delivery.as_ref())
            .or_else(|| self.recipients.first().map(|(_, d)| d))
            .cloned()
            .unwrap_or_else(|| Delivery::failed(400, "no recipients"))
    }
}

/// Everything a send needs: the deliverer and the operator's accounts.
pub struct Senders {
    deliverer: Arc<Deliverer>,
    accounts: Accounts,
}

/// Looks a config up by id; the notifier answers from its cache, the test
/// route from the store.
pub type Resolve<'a> = &'a dyn Fn(&str) -> Option<Arc<Value>>;

impl Senders {
    /// Senders over `deliverer` and the accounts an operator declared.
    #[must_use]
    pub fn new(deliverer: Arc<Deliverer>, accounts: Accounts) -> Self {
        Self {
            deliverer,
            accounts,
        }
    }

    /// Sends `text`, titled `title`, through `config`; `resolve` finds the
    /// account and the groups an email names.
    #[must_use]
    pub fn send(&self, config: &Value, title: &str, text: &str, resolve: Resolve<'_>) -> Outcome {
        match config["config_type"].as_str().unwrap_or_default() {
            "slack" | "chime" | "microsoft_teams" | "webhook" => {
                Outcome::one(self.deliverer.deliver(config, text))
            }
            "sns" => Outcome::one(aws::publish_sns(
                &self.deliverer,
                &self.accounts,
                &config["sns"],
                title,
                text,
            )),
            "email" => self.email(&config["email"], title, text, resolve),
            _ => Outcome::one(Delivery::failed(501, "not served")),
        }
    }

    fn email(&self, block: &Value, title: &str, text: &str, resolve: Resolve<'_>) -> Outcome {
        let mut to: Vec<String> = Vec::new();
        let mut add = |list: &Value| {
            for r in list.as_array().into_iter().flatten() {
                if let Some(a) = r["recipient"].as_str()
                    && !to.iter().any(|t| t == a)
                    && to.len() < MAX_RECIPIENTS
                {
                    to.push(a.to_owned());
                }
            }
        };
        add(&block["recipient_list"]);
        for id in block["email_group_id_list"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(group) = id.as_str().and_then(resolve) {
                add(&group["email_group"]["recipient_list"]);
            }
        }
        let account = block["email_account_id"].as_str().and_then(resolve);
        let Some(account) = account else {
            return Outcome::one(Delivery::failed(404, "no account"));
        };
        if to.is_empty() {
            return Outcome::one(Delivery::failed(400, "no recipients"));
        }
        let results: Vec<Delivery> = match account["config_type"].as_str() {
            Some("ses_account") => to
                .chunks(SES_BATCH)
                .flat_map(|chunk| {
                    let d = aws::send_ses(
                        &self.deliverer,
                        &self.accounts,
                        &account["ses_account"],
                        chunk,
                        title,
                        text,
                    );
                    std::iter::repeat_n(d, chunk.len())
                })
                .collect(),
            Some("smtp_account") => self.smtp(&account["smtp_account"], &to, title, text),
            _ => to
                .iter()
                .map(|_| Delivery::failed(503, "not served"))
                .collect(),
        };
        Outcome {
            delivery: None,
            recipients: to.into_iter().zip(results).collect(),
        }
    }

    /// Mail through the relay an `smtp_account` names, when the operator
    /// declared that host and port.
    fn smtp(&self, block: &Value, to: &[String], title: &str, text: &str) -> Vec<Delivery> {
        let host = block["host"].as_str().unwrap_or_default();
        let port = block["port"]
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .unwrap_or_default();
        let Some(account) = self.accounts.smtp(host, port) else {
            return to
                .iter()
                .map(|_| Delivery::failed(403, "no account"))
                .collect();
        };
        let method = block["method"].as_str().unwrap_or("none");
        let from = block["from_address"].as_str().unwrap_or_default();
        smtp::send_mail(&self.deliverer, account, method, from, to, title, text)
    }
}
