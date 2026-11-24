//! Requests for plugins that are not ported, proxied to a Java node.
//!
//! The Rust node serves no ISM, alerting, anomaly detection, ML Commons or
//! learning-to-rank endpoint; the Java shell runs those plugins, so a request
//! under their prefixes is handed to a configured Java upstream and its
//! answer handed back (ADR 0046). The proxy streams in both directions
//! through one small buffer, makes no object-store request, and is off unless
//! an upstream is set.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use oss_rest::{Exception, Length, Request, Response, RestError, Router};
use ureq::http;

/// The prefixes of the plugins whose endpoints live only in the Java shell.
pub const PLUGIN_PREFIXES: [&str; 9] = [
    "/_plugins/_ism",
    "/_opendistro/_ism",
    "/_plugins/_alerting",
    "/_opendistro/_alerting",
    "/_plugins/_anomaly_detection",
    "/_opendistro/_anomaly_detection",
    "/_plugins/_ml",
    "/_plugins/_ltr",
    "/_ltr",
];

/// How long an upstream that refused a connect is skipped.
const SKIP_MILLIS: i64 = 5_000;
/// The bytes copied from an upstream's answer at a time.
const CHUNK: usize = 16 << 10;
/// Headers never forwarded, in either direction.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

/// What the proxy is set with.
#[derive(Debug, Clone)]
pub struct ProxySettings {
    /// `http://` or `https://` bases of Java nodes, without userinfo.
    pub upstreams: Vec<String>,
    /// The most a request body may be.
    pub max_request_bytes: u64,
    /// The wait for a connect and for the answer's head, and each read after.
    pub timeout: Duration,
}

/// A setting that cannot be used.
#[derive(Debug, PartialEq, Eq)]
pub struct BadUpstream(pub String);

struct Upstream {
    base: String,
    /// Until when (epoch millis) it is skipped.
    down_until: AtomicI64,
}

/// The proxy: the upstream rotation and the HTTP agent.
pub struct PluginProxy {
    agent: ureq::Agent,
    upstreams: Vec<Upstream>,
    next: AtomicUsize,
    max_request_bytes: u64,
    start: Instant,
}

impl PluginProxy {
    /// A proxy over `settings`, or an error naming an upstream that is not an
    /// `http(s)` origin without userinfo, path or query.
    ///
    /// # Errors
    ///
    /// [`BadUpstream`] for the first upstream that cannot be used.
    pub fn new(settings: &ProxySettings) -> Result<Self, BadUpstream> {
        // ALLOC: startup only; one entry an upstream.
        let mut upstreams = Vec::new();
        for raw in &settings.upstreams {
            upstreams.push(Upstream {
                base: origin(raw)?,
                down_until: AtomicI64::new(0),
            });
        }
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_idle_connections(0)
            .user_agent(ureq::config::AutoHeaderValue::None)
            .accept(ureq::config::AutoHeaderValue::None)
            .accept_encoding(ureq::config::AutoHeaderValue::None)
            .timeout_connect(Some(settings.timeout))
            .timeout_recv_response(Some(settings.timeout))
            .timeout_per_read(Some(settings.timeout))
            .build()
            .into();
        Ok(Self {
            agent,
            upstreams,
            next: AtomicUsize::new(0),
            max_request_bytes: settings.max_request_bytes,
            start: Instant::now(),
        })
    }

    /// `router` with the prefix routes of every plugin not ported, when there
    /// is an upstream; unchanged otherwise.
    ///
    /// # Errors
    ///
    /// [`oss_rest::RouteError`] when a prefix cannot be registered.
    pub fn register(self: &Arc<Self>, mut router: Router) -> Result<Router, oss_rest::RouteError> {
        if self.upstreams.is_empty() {
            return Ok(router);
        }
        for prefix in PLUGIN_PREFIXES {
            let proxy = Arc::clone(self);
            router = router.prefix(
                prefix,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| proxy.handle(req, resp),
            )?;
        }
        Ok(router)
    }

    fn now_millis(&self) -> i64 {
        i64::try_from(self.start.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    /// The upstreams in the order to try: the next in turn first, those
    /// skipped for a recent refusal last.
    fn order(&self) -> Vec<usize> {
        let n = self.upstreams.len();
        let first = self
            .next
            .fetch_add(1, Ordering::Relaxed)
            .checked_rem(n)
            .unwrap_or(0);
        let now = self.now_millis();
        // ALLOC: one index an upstream, a handful, once a proxied request.
        let (mut up, down): (Vec<usize>, Vec<usize>) = (0..n)
            .map(|i| first.saturating_add(i).checked_rem(n).unwrap_or(0))
            .partition(|&i| self.upstreams[i].down_until.load(Ordering::Relaxed) <= now);
        up.extend(down);
        up
    }

    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        check_target(req.raw_path(), req.raw_query())?;
        let declared = req.body().declared_length();
        if declared.is_some_and(|n| n > self.max_request_bytes) {
            return Err(RestError::too_large());
        }
        // ALLOC: one chunk buffer a proxied request, CHUNK bytes, reserved first.
        req.body().reserve(CHUNK)?;
        let mut chunk = vec![0u8; CHUNK];
        let mut last = None;
        for i in self.order() {
            let upstream = &self.upstreams[i];
            match self.call(upstream, req, declared) {
                Ok(answer) => return relay(answer, resp, &mut chunk),
                Err(Failed::Connect(e)) if req.body().bytes_read() == 0 => {
                    upstream.down_until.store(
                        self.now_millis().saturating_add(SKIP_MILLIS),
                        Ordering::Relaxed,
                    );
                    last = Some(e);
                }
                Err(Failed::Connect(e) | Failed::Other(e)) => return Err(bad_gateway(&e)),
                Err(Failed::TooLarge) => return Err(RestError::too_large()),
                Err(Failed::Timeout) => return Err(timed_out()),
            }
        }
        Err(bad_gateway(
            &last.unwrap_or_else(|| "no upstream".to_owned()),
        ))
    }

    fn call(
        &self,
        upstream: &Upstream,
        req: &mut Request<'_>,
        declared: Option<u64>,
    ) -> Result<http::Response<ureq::Body>, Failed> {
        let query = req.raw_query();
        let target = if query.is_empty() {
            format!("{}{}", upstream.base, req.raw_path())
        } else {
            format!("{}{}?{query}", upstream.base, req.raw_path())
        };
        let mut builder = http::Request::builder()
            .method(req.method().as_str())
            .uri(target);
        let mut connection_named: Vec<String> = Vec::new();
        for (name, value) in req.headers() {
            if name.eq_ignore_ascii_case("connection") {
                connection_named.extend(value.split(',').map(|t| t.trim().to_ascii_lowercase()));
            }
        }
        let mut forwarded_for = None;
        for (name, value) in req.headers() {
            let lower = name.to_ascii_lowercase();
            if HOP_BY_HOP.contains(&lower.as_str())
                || lower.starts_with("proxy-")
                || connection_named.contains(&lower)
            {
                continue;
            }
            if lower == "x-forwarded-for" {
                forwarded_for = Some(value);
                continue;
            }
            builder = builder.header(name, value);
        }
        if let Some(host) = req.header("host") {
            builder = builder.header("x-forwarded-host", host);
        }
        builder = builder.header("x-forwarded-proto", "http");
        if let Some(v) = forwarded_for {
            builder = builder.header("x-forwarded-for", v);
        }
        if let Some(n) = declared {
            builder = builder.header("content-length", n);
        }
        let mut reader = BodyReader {
            body: req.body(),
            left: self.max_request_bytes,
            over: false,
            carry: Vec::new(),
        };
        let request = builder
            .body(ureq::SendBody::from_reader(&mut reader))
            .map_err(|e| Failed::Other(e.to_string()))?;
        match self.agent.run(request) {
            Ok(answer) => Ok(answer),
            Err(_) if reader.over => Err(Failed::TooLarge),
            Err(e) => Err(classify(&e)),
        }
    }
}

enum Failed {
    /// Nothing was sent: the next upstream may be tried.
    Connect(String),
    Timeout,
    TooLarge,
    Other(String),
}

fn classify(e: &ureq::Error) -> Failed {
    use io::ErrorKind::{AddrNotAvailable, ConnectionRefused, NotFound};
    use ureq::Timeout::{Connect, Resolve};
    match e {
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => Failed::Connect(e.to_string()),
        ureq::Error::Timeout(Connect | Resolve) => Failed::Connect(e.to_string()),
        ureq::Error::Timeout(_) => Failed::Timeout,
        ureq::Error::Io(io)
            if matches!(io.kind(), ConnectionRefused | NotFound | AddrNotAvailable) =>
        {
            Failed::Connect(e.to_string())
        }
        other => Failed::Other(other.to_string()),
    }
}

/// A request body as a reader, refusing a body past its limit.
struct BodyReader<'b, 'a> {
    body: &'b mut oss_rest::Body<'a>,
    left: u64,
    over: bool,
    /// What of the last chunk the caller's buffer had no room for.
    carry: Vec<u8>,
}

impl Read for BodyReader<'_, '_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.carry.is_empty() {
            let chunk = self
                .body
                .next_chunk()
                .map_err(|e| io::Error::other(format!("{e:?}")))?;
            let Some(chunk) = chunk else { return Ok(0) };
            let len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            if len > self.left {
                self.over = true;
                return Err(io::Error::other("the request body is over the limit"));
            }
            self.left = self.left.saturating_sub(len);
            // ALLOC: at most one body chunk, the body buffer's size.
            self.carry.extend_from_slice(chunk);
        }
        let n = self.carry.len().min(out.len());
        out[..n].copy_from_slice(&self.carry[..n]);
        self.carry.drain(..n);
        Ok(n)
    }
}

/// Writes the upstream's answer to the client: its status and headers, then
/// its body a chunk at a time.
fn relay(
    answer: http::Response<ureq::Body>,
    resp: &mut Response<'_>,
    chunk: &mut [u8],
) -> Result<(), RestError> {
    let (parts, mut body) = answer.into_parts();
    let mut content_type = None;
    // ALLOC: one entry a header of the answer, bounded by the upstream's head limit.
    let mut headers: Vec<(String, String)> = Vec::new();
    let named: Vec<String> = parts
        .headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();
    for (name, value) in &parts.headers {
        let lower = name.as_str();
        let Ok(value) = value.to_str() else { continue };
        if HOP_BY_HOP.contains(&lower)
            || lower.starts_with("proxy-")
            || named.iter().any(|n| n == lower)
        {
            continue;
        }
        if value.contains(['\r', '\n']) {
            continue;
        }
        if lower == "content-type" {
            content_type = Some(value.to_owned());
        } else {
            headers.push((lower.to_owned(), value.to_owned()));
        }
    }
    let length = parts
        .headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(Length::Chunked, Length::Known);
    let slice: Vec<(&str, &str)> = headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let mut out = resp.start(
        parts.status.as_u16(),
        content_type.as_deref(),
        length,
        &slice,
    )?;
    let mut reader = body.as_reader();
    loop {
        let n = reader.read(chunk).map_err(RestError::from)?;
        if n == 0 {
            break;
        }
        out.write_all(&chunk[..n]).map_err(RestError::from)?;
    }
    out.finish()
}

/// `raw` as an origin: `http(s)://host[:port]`, nothing after, no userinfo.
fn origin(raw: &str) -> Result<String, BadUpstream> {
    let bad = || BadUpstream(raw.to_owned());
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .ok_or_else(bad)?;
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ', '\\']) {
        return Err(bad());
    }
    let scheme = if raw.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    Ok(format!("{scheme}://{rest}"))
}

/// Refuses a path or query that could aim the request somewhere else.
fn check_target(path: &str, query: &str) -> Result<(), RestError> {
    let control = |s: &str| s.bytes().any(|b| b < 0x20 || b == 0x7f);
    let lower = path.to_ascii_lowercase();
    let tricky = lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%2e")
        || path.contains('\\');
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let last = segments.len().saturating_sub(1);
    let dots = segments
        .iter()
        .enumerate()
        .any(|(i, s)| *s == ".." || *s == "." || (s.is_empty() && i != last));
    if control(path) || control(query) || tricky || dots {
        return Err(RestError::exception(
            400,
            Exception::new(
                "illegal_argument_exception",
                "the request path cannot be proxied",
            ),
        ));
    }
    Ok(())
}

fn bad_gateway(why: &str) -> RestError {
    // ALLOC: error path; one message.
    RestError::exception(
        502,
        Exception::new(
            "plugin_upstream_unavailable",
            format!("no plugin upstream answered: {why}"),
        ),
    )
}

fn timed_out() -> RestError {
    RestError::exception(
        504,
        Exception::new(
            "plugin_upstream_timeout",
            "the plugin upstream did not answer in time",
        ),
    )
}
