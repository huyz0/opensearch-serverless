//! Posting a message to a channel's URL: bounded in size, time and
//! concurrency, with no redirect and no proxy (ADR 0081).

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use ureq::config::Config;
use ureq::http::Request;
use ureq::unversioned::transport::DefaultConnector;

use super::egress::{EgressPolicy, GuardedResolver, Lookup};

/// What a delivery came to, in the plugin's `delivery_status` words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// The HTTP status of the answer, or a code of this node's for a failure.
    pub status_code: u16,
    /// A word from a fixed set; never an address or a body of the target.
    pub status_text: String,
}

impl Delivery {
    fn failed(code: u16, text: &str) -> Self {
        // ALLOC: one short word, once a failed delivery.
        Self {
            status_code: code,
            status_text: text.to_owned(),
        }
    }

    /// The body of the plugin's `delivery_status`.
    #[must_use]
    pub fn shown(&self) -> Value {
        json!({"status_code": self.status_code.to_string(), "status_text": self.status_text})
    }
}

/// How many deliveries run and wait.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Deliveries running at once.
    pub running: usize,
    /// Deliveries waiting for a slot; one past it is rejected.
    pub waiting: usize,
    /// The most bytes of a message body.
    pub max_body: usize,
    /// How long a whole delivery may take.
    pub total: Duration,
    /// How long a connection may take.
    pub connect: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            running: 32,
            waiting: 256,
            max_body: 1 << 20,
            total: Duration::from_secs(30),
            connect: Duration::from_secs(5),
        }
    }
}

/// Delivers to the HTTP-shaped channels.
pub struct Deliverer {
    agent: ureq::Agent,
    policy: EgressPolicy,
    limits: Limits,
    slots: Mutex<(usize, usize)>,
    freed: Condvar,
}

impl Deliverer {
    /// A deliverer that resolves with `lookup` under `policy`.
    #[must_use]
    pub fn new(lookup: Lookup, policy: EgressPolicy, limits: Limits) -> Self {
        let config = Config::builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(None)
            .max_idle_connections(0)
            .timeout_global(Some(limits.total))
            .timeout_connect(Some(limits.connect))
            .build();
        let resolver = GuardedResolver {
            lookup,
            policy: policy.clone(),
        };
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), resolver);
        Self {
            agent,
            policy,
            limits,
            slots: Mutex::new((0, 0)),
            freed: Condvar::new(),
        }
    }

    /// Takes a running slot, waiting in the bounded queue; false past it.
    fn take(&self) -> bool {
        let mut s = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if s.0 >= self.limits.running {
            if s.1 >= self.limits.waiting {
                return false;
            }
            s.1 = s.1.saturating_add(1);
            while s.0 >= self.limits.running {
                s = self
                    .freed
                    .wait(s)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            s.1 = s.1.saturating_sub(1);
        }
        s.0 = s.0.saturating_add(1);
        true
    }

    fn give(&self) {
        let mut s = self
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        s.0 = s.0.saturating_sub(1);
        self.freed.notify_one();
    }

    /// Sends `text` to the channel `config` names. Never panics and never
    /// answers with anything of the target's but its status code.
    #[must_use]
    pub fn deliver(&self, config: &Value, text: &str) -> Delivery {
        let ty = config["config_type"].as_str().unwrap_or_default();
        let block = &config[ty];
        let Some(url) = block["url"].as_str() else {
            return Delivery::failed(400, "no url");
        };
        let key = if ty == "chime" { "Content" } else { "text" };
        // ALLOC: the body, checked against max_body before any connection.
        let body = json!({ key: text }).to_string().into_bytes();
        if body.len() > self.limits.max_body {
            return Delivery::failed(413, "too large");
        }
        let Ok(uri) = url.parse::<ureq::http::Uri>() else {
            return Delivery::failed(400, "no url");
        };
        let port = uri
            .port_u16()
            .unwrap_or(if uri.scheme_str() == Some("https") {
                443
            } else {
                80
            });
        if self
            .policy
            .refuses(uri.host().unwrap_or_default(), port)
            .is_some()
        {
            return Delivery::failed(403, "blocked");
        }
        let method = block["method"].as_str().unwrap_or("POST");
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(h) = block["header_params"].as_object() {
            for (name, value) in h {
                request = request.header(name.as_str(), value.as_str().unwrap_or_default());
            }
        }
        let Ok(request) = request.body(body) else {
            return Delivery::failed(400, "no url");
        };
        if !self.take() {
            return Delivery::failed(429, "rejected");
        }
        let answer = self.agent.run(request);
        self.give();
        match answer {
            Ok(r) => {
                let code = r.status().as_u16();
                if (200..300).contains(&code) {
                    Delivery {
                        status_code: code,
                        status_text: "Success".to_owned(),
                    }
                } else {
                    Delivery::failed(code, &format!("status {code}"))
                }
            }
            Err(ureq::Error::HostNotFound) => Delivery::failed(403, "blocked"),
            Err(ureq::Error::Timeout(_)) => Delivery::failed(504, "timeout"),
            Err(ureq::Error::ConnectionFailed | ureq::Error::Io(_)) => {
                Delivery::failed(502, "refused")
            }
            Err(_) => Delivery::failed(502, "failed"),
        }
    }
}
