//! The one client a connector's request goes out through: https only, no redirect, no proxy,
//! every address the name resolves to checked and the connection made to one that was, a
//! bound on time, on response bytes and on calls in flight (the notification egress guard).

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use oss_ml::predict::Failure;
use ureq::config::Config;
use ureq::http::Request;
use ureq::unversioned::transport::DefaultConnector;

use crate::notifications_route::{EgressPolicy, GuardedResolver, Lookup};

/// The bounds of a remote call.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Calls in flight on this node.
    pub running: usize,
    /// Calls waiting for a slot; one past it is 429.
    pub waiting: usize,
    /// The most bytes of a response that are read.
    pub max_response: u64,
    /// How long a whole call may take.
    pub total: Duration,
    /// How long a connection may take.
    pub connect: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            running: 32,
            waiting: 256,
            max_response: 10 << 20,
            total: Duration::from_secs(30),
            connect: Duration::from_secs(5),
        }
    }
}

/// The operator's rules for where connectors may connect: public https hosts on port 443
/// unless `OSS_ML_PRIVATE_ENDPOINTS=true` (private addresses and plain http, for a test
/// rig or a private model server) and `OSS_ML_PORTS` (a comma list).
#[derive(Clone, Debug)]
pub struct Policy {
    /// The address and port rules.
    pub egress: EgressPolicy,
    /// Whether plain `http` URLs may be called.
    pub allow_http: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            egress: EgressPolicy {
                allow_private: false,
                ports: vec![443],
                deny_hosts: Vec::new(),
            },
            allow_http: false,
        }
    }
}

impl Policy {
    /// The policy of the node's environment.
    #[must_use]
    pub fn from_env() -> Self {
        let mut p = Self::default();
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        if get("OSS_ML_PRIVATE_ENDPOINTS").is_some_and(|v| v == "true") {
            p.egress.allow_private = true;
            p.allow_http = true;
        }
        if let Some(v) = get("OSS_ML_PORTS") {
            let ports: Vec<u16> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if !ports.is_empty() {
                p.egress.ports = ports;
            }
        }
        p
    }
}

/// What came back from a call.
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The body, at most the limit.
    pub body: String,
}

/// Sends the requests connectors make.
pub struct Outbound {
    agent: ureq::Agent,
    allow_http: bool,
    limits: Limits,
    slots: Mutex<(usize, usize)>,
    freed: Condvar,
}

fn failure(status: u16, reason: &str) -> Failure {
    Failure {
        status,
        // ALLOC: error path; one short message.
        reason: reason.to_owned(),
    }
}

impl Outbound {
    /// A client resolving with `lookup` under `policy`.
    #[must_use]
    pub fn new(lookup: Lookup, policy: Policy, limits: Limits) -> Self {
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
            policy: policy.egress,
        };
        Self {
            agent: ureq::Agent::with_parts(config, DefaultConnector::default(), resolver),
            allow_http: policy.allow_http,
            limits,
            slots: Mutex::new((0, 0)),
            freed: Condvar::new(),
        }
    }

    /// Takes a slot, waiting in the bounded queue; false past it.
    fn take(&self) -> bool {
        let mut s = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        if s.0 >= self.limits.running {
            if s.1 >= self.limits.waiting {
                return false;
            }
            s.1 = s.1.saturating_add(1);
            while s.0 >= self.limits.running {
                s = self.freed.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
            s.1 = s.1.saturating_sub(1);
        }
        s.0 = s.0.saturating_add(1);
        true
    }

    fn give(&self) {
        let mut s = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        s.0 = s.0.saturating_sub(1);
        self.freed.notify_one();
    }

    /// Sends one request. The refusals are the caller's to show: a URL that is not https, a
    /// host or address the policy refuses (400, never saying which address), a redirect, a
    /// timeout (408), a refused connection (502), a response past the limit (502).
    ///
    /// # Errors
    /// As above.
    pub fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<Answer, Failure> {
        let uri: ureq::http::Uri = url
            .parse()
            .map_err(|_| failure(400, "the connector URL is not valid"))?;
        let scheme = uri.scheme_str().unwrap_or_default();
        if scheme != "https" && !(scheme == "http" && self.allow_http) {
            return Err(failure(400, "the connector URL must use https"));
        }
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let request = request
            .body(body)
            .map_err(|_| failure(400, "the connector request is not valid"))?;
        if !self.take() {
            return Err(failure(
                429,
                "too many calls to remote services are in flight",
            ));
        }
        let answer = self.agent.run(request);
        self.give();
        let response = answer.map_err(|e| match e {
            ureq::Error::HostNotFound => failure(400, "the connector's host is not allowed"),
            ureq::Error::Timeout(_) => failure(408, "the remote service did not answer in time"),
            _ => failure(502, "the remote service could not be reached"),
        })?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(failure(502, "the remote service answered with a redirect"));
        }
        let mut body = response.into_body();
        let text = body
            .with_config()
            .limit(self.limits.max_response)
            .read_to_string()
            .map_err(|_| {
                failure(
                    502,
                    "the remote service's response is too large or unreadable",
                )
            })?;
        Ok(Answer { status, body: text })
    }
}
