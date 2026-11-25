//! One connection: read a head, route it, stream the body to the handler,
//! repeat while the connection stays open.
//!
//! The order of the checks follows where OpenSearch makes them, at
//! huyz0/OpenSearch@49f238ed: framing and size first (Netty's decoder and
//! `HttpObjectAggregator`, 400, 413 and 417), then routing
//! (`RestController#tryAllHandlers`, 400 and 405), then the content type
//! (`RestController#dispatchRequest`, 406) and query parameters
//! (`BaseRestHandler#handleRequest`, 400). This server then takes a body
//! buffer (429 after a bounded wait) and only then sends `100 Continue`, so a
//! client that waits for it never uploads a body that will be refused.
//!
//! A handler that panics is answered 500 and its connection closed; the
//! worker carries on with the next connection.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::Ordering;
use std::time::Instant;

use oss_mem::PooledBuffer;

use crate::admission::{Decision, Permit};
use crate::authorizer::Authorization;
use crate::body::{BREAKER_LABEL, Body, Framing};
use crate::checks::{check_params, content_type, java_parses_uri, plan};
use crate::config::ServerConfig;
use crate::error::{Exception, RestError};
use crate::guard::{Admission, Denial, Verdict};
use crate::head::{self, Head, HeadLimits, Scan, Version};
use crate::inbound::Inbound;
use crate::linger::linger;
use crate::method::{Method, MethodSet};
use crate::query::{QueryParams, decode_component};
use crate::request::Request;
use crate::response::{Length, Response};
use crate::router::{MAX_SEGMENTS, PathParam, Resolved};
pub(crate) use crate::shared::{ParkTicket, Replay, Shared};
use crate::timed::Timed;

/// Query parameters every route takes: they only change how a response is
/// formatted. `pretty` indentation and `human` units are not applied yet.
pub(crate) const GLOBAL_PARAMS: [&str; 2] = ["pretty", "human"];

/// The most unread body bytes read and dropped after a handler to keep the
/// connection open; past this it closes instead.
const DRAIN_LIMIT: u64 = 64 << 10;

/// The content type of an empty `OPTIONS` answer.
const TEXT: &str = "text/plain; charset=UTF-8";

/// One worker's buffers and scratch, allocated once when it starts.
pub(crate) struct Worker {
    pub(crate) head_buf: PooledBuffer,
    pub(crate) read_buf: PooledBuffer,
    pub(crate) write_buf: PooledBuffer,
    pub(crate) head: Head,
    pub(crate) query: QueryParams,
    pub(crate) bound: Vec<PathParam>,
}

impl Worker {
    pub(crate) fn new(
        config: &ServerConfig,
        head_buf: PooledBuffer,
        read_buf: PooledBuffer,
        write_buf: PooledBuffer,
    ) -> Self {
        Self {
            head_buf,
            read_buf,
            write_buf,
            head: Head::new(config.max_headers),
            // ALLOC: once per worker; `max_query_params` pairs.
            query: QueryParams::with_capacity(config.max_query_params),
            // ALLOC: once per worker; a route binds at most MAX_SEGMENTS.
            bound: Vec::with_capacity(MAX_SEGMENTS),
        }
    }
}

/// Serves `stream` until it closes, times out or must yield, then closes it
/// gracefully: our side shuts for writing, and input already in flight is
/// read and discarded (at most `LINGER_LIMIT` bytes, for at most
/// `LINGER_TIME`, stopping after `LINGER_IDLE` of quiet) so it cannot
/// turn the close into a reset.
pub(crate) fn serve(
    w: &mut Worker,
    stream: &TcpStream,
    shared: &Shared,
    replay: Option<Replay>,
) -> Option<ParkTicket> {
    if let Some(ticket) = serve_requests(w, stream, shared, replay) {
        return Some(ticket);
    }
    let _ = linger(stream);
    None
}

/// The request loop [`serve`] runs before it closes.
fn serve_requests(
    w: &mut Worker,
    stream: &TcpStream,
    shared: &Shared,
    replay: Option<Replay>,
) -> Option<ParkTicket> {
    let _ = stream.set_nodelay(true);
    let c = &shared.config;
    let timed = Timed::new(stream, c.slow_grace, c.min_transfer_rate);
    let Worker {
        head_buf,
        read_buf,
        write_buf,
        head,
        query,
        bound,
    } = w;
    let mut inbound = Inbound::new(&timed, &mut read_buf[..]);
    let mut served = false;
    // The permit a resumed request already holds, used by its first request.
    let mut reserved = None;
    if let Some(r) = replay {
        if inbound.unread(&r.bytes).is_err() {
            return None;
        }
        reserved = Some(r.permit);
    }
    loop {
        let (end, filled) = match read_head(&mut inbound, head_buf, stream, shared, served) {
            Ok(Some(found)) => found,
            Ok(None) => return None,
            Err(e) => {
                timed.restart();
                let mut out = &timed;
                let mut resp = Response::new(&mut out, write_buf, false, Some("close"), None, None);
                let _ = resp.send_error(&e);
                return None;
            }
        };
        if inbound.unread(&head_buf[end..filled]).is_err() {
            return None;
        }
        timed.restart();
        let mut permit = None;
        let mut park = None;
        let ctx = Ctx {
            head_buf: &head_buf[..end],
            head,
            query,
            bound,
            write_buf,
            shared,
            reserved: reserved.take(),
            permit: &mut permit,
            park: &mut park,
        };
        let keep = handle(ctx, &mut inbound, &timed);
        if let Some(tenant) = park {
            // ALLOC: one copy of the head and read-ahead, at most one head
            // buffer, for a request that waits in the queue.
            let bytes = head_buf[..filled].to_vec();
            return Some(ParkTicket { tenant, bytes });
        }
        if permit.take().is_some()
            && let Some(queue) = &shared.queue
        {
            // The permit is back: a parked search may run.
            queue.offer();
        }
        if !keep {
            return None;
        }
        served = true;
    }
}

/// Reads a complete head into `buf`, returning where it ends and how much
/// was read; `None` when the connection should close quietly, which it does
/// when a started head takes longer than `request_timeout` to complete.
fn read_head(
    inbound: &mut Inbound<&Timed<'_>, &mut [u8]>,
    buf: &mut [u8],
    stream: &TcpStream,
    shared: &Shared,
    keep_alive: bool,
) -> Result<Option<(usize, usize)>, RestError> {
    let c = &shared.config;
    let limits = HeadLimits {
        max_initial_line: c.max_initial_line,
        max_header_bytes: c.max_header_bytes,
    };
    let mut filled = inbound.take_buffered(buf);
    let started = Instant::now();
    let mut first_byte = (filled > 0).then_some(started);
    let mut src = stream;
    loop {
        if let Scan::Complete(end) = head::scan(&buf[..filled], &limits)? {
            return Ok(Some((end, filled)));
        }
        if filled == buf.len() {
            return Err(RestError::illegal_argument("HTTP head larger than its buffer").closing());
        }
        let timeout = match first_byte {
            None => c.idle_poll,
            Some(t) => match c.request_timeout.checked_sub(t.elapsed()) {
                Some(left) if !left.is_zero() => left,
                _ => return Ok(None),
            },
        };
        let _ = stream.set_read_timeout(Some(timeout));
        match src.read(&mut buf[filled..]) {
            Ok(0) => return Ok(None),
            Ok(n) => {
                filled = filled.saturating_add(n);
                first_byte.get_or_insert_with(Instant::now);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if is_timeout(&e) && filled == 0 => {
                let idle = started.elapsed();
                let limit = if keep_alive {
                    c.idle_timeout
                } else {
                    c.request_timeout
                };
                let yield_to_waiting = keep_alive && shared.has_waiting();
                if shared.shutdown.load(Ordering::Acquire) || yield_to_waiting || idle >= limit {
                    return Ok(None);
                }
            }
            Err(_) => return Ok(None),
        }
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

fn has_token(value: Option<&str>, token: &str) -> bool {
    value.is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
}

struct Ctx<'w> {
    head_buf: &'w [u8],
    head: &'w mut Head,
    query: &'w mut QueryParams,
    bound: &'w mut Vec<PathParam>,
    write_buf: &'w mut [u8],
    shared: &'w Shared,
    /// The permit a resumed request holds, for its first request.
    reserved: Option<Permit>,
    /// Where `handle` leaves the permit its request holds.
    permit: &'w mut Option<Permit>,
    /// Where `handle` leaves the tenant a request is parked as.
    park: &'w mut Option<std::sync::Arc<str>>,
}

/// Handles one request; whether the connection stays open.
fn handle(ctx: Ctx<'_>, inbound: &mut Inbound<&Timed<'_>, &mut [u8]>, stream: &Timed<'_>) -> bool {
    let Ctx {
        head_buf: buf,
        head,
        query: params,
        bound,
        write_buf,
        shared,
        reserved,
        permit,
        park,
    } = ctx;
    let mut out = stream;
    if let Err(e) = head.parse(buf, buf.len()) {
        let mut resp = Response::new(&mut out, write_buf, false, Some("close"), None, None);
        let _ = resp.send_error(&e);
        return false;
    }
    let head: &Head = head;
    let method = Method::parse(&buf[head.method.clone()]);
    let target = std::str::from_utf8(&buf[head.target.clone()]).unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let conn = head.header(buf, "connection");
    let wants_open = match head.version {
        Version::Http11 => !has_token(conn, "close"),
        Version::Http10 => has_token(conn, "keep-alive"),
    };
    let connection = match (wants_open, head.version) {
        (false, _) => Some("close"),
        (true, Version::Http10) => Some("keep-alive"),
        (true, Version::Http11) => None,
    };
    let head_only = method == Some(Method::Head);
    let opaque = head.header(buf, "x-opaque-id");
    let request_id = head.header(buf, "x-request-id");
    let mut resp = Response::new(
        &mut out, write_buf, head_only, connection, opaque, request_id,
    );
    if head.version == Version::Http10 {
        resp.set_http10();
    }

    let plan = match plan(head, buf, &shared.config) {
        Ok(p) => p,
        Err(e) => return refuse(&mut resp, &e, wants_open, false),
    };
    let has_body = plan.framing != Framing::Empty;
    let mut principal = None;
    if let Some(guard) = shared.router.guard() {
        let admission = Admission {
            method,
            path,
            authorization: head.header(buf, "authorization"),
        };
        match guard.admit(&admission) {
            Verdict::Allow(who) => principal = who,
            Verdict::Deny(denial) => return deny(&mut resp, &denial, wants_open, has_body),
        }
    }
    let route = match shared.router.resolve(method, path, bound) {
        Resolved::Found(route) => route,
        Resolved::OtherMethods(allowed) if method == Some(Method::Options) => {
            // ALLOC: OPTIONS only; the method names, under 64 bytes.
            let mut allow = String::new();
            let _ = allowed.write_joined(&mut allow, ",");
            let sent = resp
                .start(200, Some(TEXT), Length::Known(0), &[("allow", &allow)])
                .and_then(|w| w.finish());
            return sent.is_ok() && wants_open && !has_body;
        }
        Resolved::NotFound if method == Some(Method::Options) => {
            let sent = resp.send(200, TEXT, b"");
            return sent.is_ok() && wants_open && !has_body;
        }
        Resolved::OtherMethods(allowed) => {
            let e = RestError::method_not_allowed(target, method, allowed);
            return refuse(&mut resp, &e, wants_open, has_body);
        }
        Resolved::NotFound => {
            let e = match method {
                Some(m) => match decode_component(path, false) {
                    Ok(p) if java_parses_uri(target) => RestError::no_handler(&p, m),
                    _ => RestError::invalid_uri(),
                },
                None => RestError::method_not_allowed(target, None, MethodSet::EMPTY),
            };
            return refuse(&mut resp, &e, wants_open, has_body);
        }
    };
    if let Some(authorizer) = shared.router.authorizer() {
        let who = principal.as_deref();
        let authorization = Authorization {
            principal: who,
            method,
            pattern: route.pattern(),
            prefix: route.is_prefix(),
            path,
            params: bound,
        };
        if let Some(denial) = authorizer.authorize(&authorization) {
            return deny(&mut resp, &denial, wants_open, has_body);
        }
    }
    if has_body && let Err(e) = content_type(head, buf) {
        return refuse(&mut resp, &e, wants_open, true);
    }
    if let Err(e) = check_params(params, bound, route, path, query) {
        return refuse(&mut resp, &e, wants_open, has_body);
    }
    if let Some(queue) = &shared.queue {
        match reserved {
            Some(held) => *permit = Some(held),
            None => match queue.admission().decide(method, path) {
                Decision::Run(held) => *permit = held,
                Decision::Defer(tenant) => {
                    *park = Some(tenant);
                    return false;
                }
            },
        }
    }
    let mut buffer = None;
    if has_body {
        match shared.bodies.checkout(shared.config.body_wait) {
            Ok(b) => buffer = Some(b),
            Err(e) => {
                let e = RestError::circuit_breaking(&e, BREAKER_LABEL).closing();
                return refuse(&mut resp, &e, wants_open, true);
            }
        }
        if plan.expect_continue && stream_write(stream, b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
            return false;
        }
    }
    let c = &shared.config;
    let body = Body::new(
        inbound,
        plan.framing,
        buffer,
        c.max_content_length,
        c.max_header_bytes,
        &shared.budget,
    );
    let mut req = Request {
        method: method.unwrap_or(Method::Get),
        path,
        query,
        params,
        path_params: bound,
        head,
        principal,
        head_buf: buf,
        body,
    };
    let handled = panic::catch_unwind(AssertUnwindSafe(|| {
        route.handler().handle(&mut req, &mut resp)
    }));
    let Ok(result) = handled else {
        drop(req);
        if !resp.is_started() {
            let e = RestError::exception(
                500,
                Exception::new("illegal_state_exception", "the handler panicked"),
            )
            .closing();
            let _ = resp.send_error(&e);
        }
        return false;
    };
    // A handler that stopped reading early (or never read) leaves the rest;
    // a small rest is read and dropped so the connection stays usable.
    let body_done = req.body.is_done() || req.body.drain(DRAIN_LIMIT);
    drop(req);
    match result {
        Ok(()) if resp.is_finished() => wants_open && body_done && !resp.closes(),
        Ok(()) => {
            if !resp.is_started() {
                let e = RestError::exception(
                    500,
                    Exception::new("illegal_state_exception", "the handler sent no response"),
                );
                let _ = resp.send_error(&e);
            }
            false
        }
        Err(e) if !resp.is_started() => refuse(&mut resp, &e, wants_open, !body_done),
        Err(_) => false,
    }
}

/// Sends a guard's or an authorizer's refusal. A refused request with a body
/// closes the connection, since the body is not read.
fn deny(resp: &mut Response<'_>, denial: &Denial, wants_open: bool, has_body: bool) -> bool {
    let challenge = denial.challenge.map(|c| [("www-authenticate", c)]);
    let sent = resp
        .start(
            denial.status,
            Some(denial.content_type),
            Length::Known(denial.body.len() as u64),
            challenge.as_ref().map_or(&[], |c| c.as_slice()),
        )
        .and_then(|mut w| {
            w.write_all(denial.body.as_bytes())?;
            w.finish()
        });
    sent.is_ok() && wants_open && !has_body
}

/// Sends `e`; whether the connection stays open.
fn refuse(resp: &mut Response<'_>, e: &RestError, wants_open: bool, body_unread: bool) -> bool {
    resp.send_error(e).is_ok() && wants_open && !body_unread && !e.closes_connection()
}

fn stream_write(stream: &Timed<'_>, bytes: &[u8]) -> io::Result<()> {
    let mut s = stream;
    s.write_all(bytes)?;
    s.flush()
}
