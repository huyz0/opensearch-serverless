//! The Java shell's `TransportAuthenticator`: each forwarded request carries
//! an HMAC over its action, sender, recipient, a timestamp, a nonce, a
//! digest of the request and a digest of its headers, keyed by a secret the
//! register rotates.
//!
//! Header value: `generation:sender:timestamp:nonce:mac`, nonce and mac in
//! hex. A receiver accepts the current generation, or the previous one for
//! one window after a rotation; a timestamp within the window of now; and
//! each nonce once. Unlike the Java receiver, a Rust node does not accept
//! the legacy static token: a request without a MAC is refused.
//!
//! The secrets come from a [`SecretSource`] (the register is read by the
//! layer that owns object-store access); this module makes no request.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::exception::RemoteException;
use crate::server::{Handler, Request};
use crate::stream::Output;

/// The header a forwarded request carries its authentication in.
pub const MAC_HEADER: &str = "x-serverless-transport-mac";

/// The Java's legacy static-token header, left out of the headers digest.
pub const TOKEN_HEADER: &str = "x-serverless-transport-token";

/// The tag the MAC begins with.
const TAG: &str = "serverless-transport-v2";

/// The least time between two secret probes for an unseen generation.
const PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// The transport secrets as the register holds them (`TransportSecrets`).
#[derive(Clone, PartialEq, Eq)]
pub struct Secrets {
    /// The register's generation.
    pub generation: i64,
    /// The current key, as hex.
    pub current: String,
    /// The previous key, as hex, accepted for one window after a rotation.
    pub previous: Option<String>,
    /// When the current key replaced the previous, in epoch milliseconds.
    pub rotated_at_millis: i64,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the keys.
        f.debug_struct("Secrets")
            .field("generation", &self.generation)
            .field("rotated_at_millis", &self.rotated_at_millis)
            .finish_non_exhaustive()
    }
}

/// Where the authenticator reads the secrets.
pub trait SecretSource: Send + Sync {
    /// The secrets as this node last read them, or `None` before it has.
    fn secrets(&self) -> Option<Arc<Secrets>>;

    /// Re-reads the secrets from the register, for a generation this node
    /// has not seen. Called at most once a second.
    fn refresh(&self);
}

/// How the authenticator runs.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// This process's identity, `node_id/ephemeral_id`: the sender it
    /// signs as and the recipient it verifies for.
    pub identity: String,
    /// How far a timestamp may be from now, and how long a nonce is
    /// remembered: one lease TTL.
    pub window_millis: i64,
    /// The most nonces remembered; a full cache refuses rather than evicts.
    pub replay_capacity: usize,
}

/// A process's identity as the MAC names it: `node_id/ephemeral_id`.
pub fn identity(node_id: &str, ephemeral_id: &str) -> String {
    // ALLOC: one identity per peer called or per authenticator.
    format!("{node_id}/{ephemeral_id}")
}

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Signs and verifies forwarded requests.
pub struct Authenticator {
    config: AuthConfig,
    source: Arc<dyn SecretSource>,
    clock: Clock,
    nonces: Mutex<NonceCache>,
    probe: Mutex<Probe>,
}

impl std::fmt::Debug for Authenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Authenticator")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Probe {
    running: bool,
    last: Option<Instant>,
}

impl Authenticator {
    /// An authenticator reading `source`, with `clock` giving epoch
    /// milliseconds (the plane's clock).
    pub fn new(config: AuthConfig, source: Arc<dyn SecretSource>, clock: Clock) -> Self {
        let capacity = config.replay_capacity.max(1);
        Self {
            config,
            source,
            clock,
            nonces: Mutex::new(NonceCache::new(capacity)),
            probe: Mutex::new(Probe::default()),
        }
    }

    /// The header value for a request to `recipient` (an [`identity`]),
    /// whose `writeTo` bytes are `request` and whose headers as sent are
    /// `headers`; `None` before this node has a secret.
    pub fn sign(
        &self,
        action: &str,
        request: &[u8],
        recipient: &str,
        headers: &[(String, String)],
    ) -> Option<String> {
        let secrets = self.source.secrets()?;
        let now = (self.clock)();
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).ok()?;
        let nonce = hex(&nonce);
        let sender = &self.config.identity;
        let key = unhex(&secrets.current)?;
        let mac = mac(
            &key,
            &Fields {
                action,
                sender,
                recipient,
                timestamp: now,
                nonce: &nonce,
            },
            request,
            headers,
        )?;
        // ALLOC: one header value per request signed.
        Some(format!(
            "{}:{sender}:{now}:{nonce}:{}",
            secrets.generation,
            hex(&mac)
        ))
    }

    /// Refuses unless `header` authenticates this exact request, to this
    /// node, now, once.
    ///
    /// # Errors
    ///
    /// `OpenSearchSecurityException` (403) for anything that does not
    /// verify; a rejection (429) when a secret probe for an unseen
    /// generation is already running or ran within the last second.
    pub fn verify(
        &self,
        action: &str,
        request: &[u8],
        header: Option<&str>,
        headers: &[(String, String)],
    ) -> Result<(), RemoteException> {
        let header =
            header.ok_or_else(|| refuse("forwarded request without transport authentication"))?;
        let parsed = Parsed::read(header)
            .ok_or_else(|| refuse("malformed transport authentication header"))?;
        let now = (self.clock)();
        let window = self.config.window_millis;
        let age = now
            .checked_sub(parsed.timestamp)
            .ok_or_else(|| refuse("transport authentication outside the time window"))?;
        if age < window.saturating_neg() || age > window {
            return Err(refuse("transport authentication outside the time window"));
        }
        let mut secrets = self
            .source
            .secrets()
            .ok_or_else(|| refuse("this node has no transport secret yet"))?;
        if parsed.generation > secrets.generation {
            if self.begin_probe() {
                self.source.refresh();
                self.end_probe();
            } else if self
                .source
                .secrets()
                .is_some_and(|latest| parsed.generation > latest.generation)
            {
                return Err(RemoteException::rejected(
                    "transport secret refresh is pending or rate limited; retry after one second",
                ));
            }
            secrets = self
                .source
                .secrets()
                .ok_or_else(|| refuse("this node has no transport secret yet"))?;
        }
        let key = if parsed.generation == secrets.generation {
            Some(secrets.current.as_str())
        } else if Some(parsed.generation) == secrets.generation.checked_sub(1)
            && now.saturating_sub(secrets.rotated_at_millis) <= window
        {
            secrets.previous.as_deref()
        } else {
            None
        };
        let key = key
            .and_then(unhex)
            .ok_or_else(|| refuse("transport authentication under an unknown secret generation"))?;
        let expected = mac(
            &key,
            &Fields {
                action,
                sender: parsed.sender,
                recipient: &self.config.identity,
                timestamp: parsed.timestamp,
                nonce: parsed.nonce,
            },
            request,
            headers,
        )
        .ok_or_else(|| refuse("forwarded request without a valid transport token"))?;
        if !constant_time_eq(&expected, &parsed.mac) {
            // The Java's words, so a test that pins the refusal holds on both.
            return Err(refuse("forwarded request without a valid transport token"));
        }
        // ALLOC: one replay key per request verified, kept at most one window.
        let replay_key = format!("{}:{}", parsed.sender, parsed.nonce);
        let remembered = self
            .nonces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remember(replay_key, parsed.timestamp, now, window);
        if remembered {
            Ok(())
        } else {
            Err(refuse(
                "replayed forwarded request or transport replay cache full",
            ))
        }
    }

    fn begin_probe(&self) -> bool {
        let mut probe = self.probe.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let recent = probe
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < PROBE_INTERVAL);
        if probe.running || recent {
            return false;
        }
        probe.last = Some(now);
        probe.running = true;
        true
    }

    fn end_probe(&self) {
        self.probe
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running = false;
    }
}

/// A handler that verifies each request's MAC before the inner one runs.
pub struct Guarded<H> {
    auth: Arc<Authenticator>,
    inner: H,
}

impl<H> Guarded<H> {
    /// Guards `inner` with `auth`.
    pub fn new(auth: Arc<Authenticator>, inner: H) -> Self {
        Self { auth, inner }
    }
}

impl<H: Handler> Handler for Guarded<H> {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let headers = &request.variable.request_headers;
        self.auth.verify(
            request.action,
            request.message,
            request.variable.header(MAC_HEADER),
            headers,
        )?;
        self.inner.handle(request, out)
    }
}

struct Fields<'a> {
    action: &'a str,
    sender: &'a str,
    recipient: &'a str,
    timestamp: i64,
    nonce: &'a str,
}

/// The MAC: the tag and fields NUL-terminated, then the request's digest,
/// then the headers' digest.
fn mac(
    key: &[u8],
    fields: &Fields<'_>,
    request: &[u8],
    headers: &[(String, String)],
) -> Option<[u8; 32]> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).ok()?;
    // ALLOC: one decimal timestamp, at most 20 bytes.
    let timestamp = fields.timestamp.to_string();
    for field in [
        TAG,
        fields.action,
        fields.sender,
        fields.recipient,
        timestamp.as_str(),
        fields.nonce,
    ] {
        mac.update(field.as_bytes());
        mac.update(&[0]);
    }
    mac.update(&Sha256::digest(request));
    mac.update(&headers_digest(headers)?);
    Some(mac.finalize().into_bytes().into())
}

/// SHA-256 of `writeMap` over the headers sorted as a Java `TreeMap` sorts
/// strings (by UTF-16 units), without the MAC and token headers.
fn headers_digest(headers: &[(String, String)]) -> Option<[u8; 32]> {
    // Reversed, so that after a stable sort the first of equal keys is the
    // last sent, as a Java map read from the wire keeps.
    // ALLOC: references to the request's headers, a handful.
    let mut kept: Vec<&(String, String)> = headers
        .iter()
        .rev()
        .filter(|(k, _)| k != MAC_HEADER && k != TOKEN_HEADER)
        // ALLOC: as above.
        .collect();
    kept.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
    kept.dedup_by(|later, earlier| later.0 == earlier.0);
    // ALLOC: the headers' encoding, a few hundred bytes.
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, usize::MAX);
    out.count(kept.len()).ok()?;
    for (key, value) in kept {
        out.string(key).ok()?;
        out.string(value).ok()?;
    }
    Some(Sha256::digest(&buf).into())
}

/// A header value, split.
struct Parsed<'a> {
    generation: i64,
    sender: &'a str,
    timestamp: i64,
    nonce: &'a str,
    mac: Vec<u8>,
}

impl<'a> Parsed<'a> {
    fn read(header: &'a str) -> Option<Self> {
        let mut parts = header.split(':');
        let generation = parts.next()?.parse().ok()?;
        let sender = parts.next()?;
        let timestamp = parts.next()?.parse().ok()?;
        let nonce = parts.next()?;
        let mac = parts.next()?;
        // The Java's split drops a trailing empty part, so an empty mac is
        // malformed there too.
        if parts.next().is_some() || mac.is_empty() {
            return None;
        }
        Some(Self {
            generation,
            sender,
            timestamp,
            nonce,
            mac: unhex(mac)?,
        })
    }
}

fn refuse(why: &str) -> RemoteException {
    RemoteException::security(why, "FORBIDDEN")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    // ALLOC: two chars a byte, for a nonce or a mac.
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

/// Hex to bytes, `None` for an odd length or a non-hex digit.
fn unhex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| {
        char::from(c)
            .to_digit(16)
            .and_then(|d| u8::try_from(d).ok())
    };
    // ALLOC: one MAC's bytes, 32 for a well-formed header.
    digits
        .chunks_exact(2)
        .map(|pair| match pair {
            [high, low] => Some((digit(*high)? << 4) | digit(*low)?),
            _ => None,
        })
        // ALLOC: as above.
        .collect()
}

/// Nonces accepted, each kept until its signed timestamp leaves the window.
/// A full cache refuses new frames rather than evict a nonce that could
/// still be replayed.
struct NonceCache {
    table: HashSet<String>,
    expirations: BinaryHeap<Reverse<(i64, String)>>,
    capacity: usize,
}

impl NonceCache {
    fn new(capacity: usize) -> Self {
        Self {
            // ALLOC: grows to the replay capacity at most.
            table: HashSet::new(),
            // ALLOC: as above.
            expirations: BinaryHeap::new(),
            capacity,
        }
    }

    fn remember(&mut self, nonce: String, timestamp: i64, now: i64, window: i64) -> bool {
        while self
            .expirations
            .peek()
            .is_some_and(|Reverse((expires, _))| *expires < now)
        {
            if let Some(Reverse((_, old))) = self.expirations.pop() {
                self.table.remove(&old);
            }
        }
        let Some(expires) = timestamp.checked_add(window) else {
            return false;
        };
        if expires < now || self.table.contains(&nonce) || self.table.len() >= self.capacity {
            return false;
        }
        self.table.insert(nonce.clone());
        self.expirations.push(Reverse((expires, nonce)));
        true
    }
}
