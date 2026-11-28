//! Posting a message to a channel's URL: bounded in size, time and
//! concurrency, with no redirect and no proxy (ADR 0081).

use std::sync::{Arc, Condvar, Mutex};
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
    pub(super) fn failed(code: u16, text: &str) -> Self {
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
    lookup: Lookup,
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
            lookup: Arc::clone(&lookup),
            policy: policy.clone(),
        };
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), resolver);
        Self {
            agent,
            lookup,
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
        let mut headers: Vec<(String, String)> =
            vec![("content-type".to_owned(), "application/json".to_owned())];
        if let Some(h) = block["header_params"].as_object() {
            for (name, value) in h {
                headers.push((name.clone(), value.as_str().unwrap_or_default().to_owned()));
            }
        }
        self.exchange(method, uri, &headers, body)
    }

    /// Posts `body` to `url` under the same bounds as a channel delivery: the
    /// egress guard, the slots, the time and no redirect. The answer is a
    /// status; nothing the target sent back is kept.
    #[must_use]
    pub(super) fn post(&self, url: &str, headers: &[(String, String)], body: Vec<u8>) -> Delivery {
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
        self.exchange("POST", uri, headers, body)
    }

    /// Opens a connection to `host` and `port` under the same rules as a
    /// delivery: the policy on the name, the lookup, every address checked, the
    /// connect and total time bounds. The refusal is a status, never an address.
    ///
    /// The host and port are one an operator declared, so the webhook port
    /// list does not apply; the name rules and the address rules do. The
    /// attempts share `deadline`.
    pub(super) fn connect(
        &self,
        host: &str,
        port: u16,
        deadline: std::time::Instant,
    ) -> Result<std::net::TcpStream, Delivery> {
        use std::net::{IpAddr, TcpStream};
        if self.policy.refuses_host(host).is_some() {
            return Err(Delivery::failed(403, "blocked"));
        }
        let addrs = match host.parse::<IpAddr>() {
            Ok(ip) => vec![std::net::SocketAddr::new(ip, port)],
            Err(_) => (self.lookup)(host, port).map_err(|_| Delivery::failed(403, "blocked"))?,
        };
        if addrs.is_empty()
            || addrs
                .iter()
                .any(|a| !super::egress::permitted(a.ip(), self.policy.allow_private))
        {
            return Err(Delivery::failed(403, "blocked"));
        }
        let stream = addrs
            .iter()
            .take(16)
            .find_map(|a| {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                let wait = left.min(self.limits.connect);
                (!wait.is_zero())
                    .then(|| TcpStream::connect_timeout(a, wait).ok())
                    .flatten()
            })
            .ok_or_else(|| Delivery::failed(502, "refused"))?;
        let _ = stream.set_nodelay(true);
        Ok(stream)
    }

    /// Runs `work` in a slot; none when the queue is full.
    pub(super) fn slotted<T>(&self, work: impl FnOnce() -> T) -> Option<T> {
        if !self.take() {
            return None;
        }
        // Frees the slot even if `work` panics.
        struct Held<'a>(&'a Deliverer);
        impl Drop for Held<'_> {
            fn drop(&mut self) {
                self.0.give();
            }
        }
        let _held = Held(self);
        Some(work())
    }

    /// The most bytes of a message.
    pub(super) fn max_body(&self) -> usize {
        self.limits.max_body
    }

    /// How long a whole send may take.
    pub(super) fn total(&self) -> Duration {
        self.limits.total
    }

    /// One request, once a slot is free; the guard has already looked at the
    /// address and the resolver looks again at the name.
    fn exchange(
        &self,
        method: &str,
        uri: ureq::http::Uri,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Delivery {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
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
