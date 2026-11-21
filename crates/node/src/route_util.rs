//! What the cluster, health and cat routes share: a refusal as the REST
//! layer's error, a flag read as the shell reads one, and a table sent.

use oss_coordinator::cat::table::Answer;
use oss_coordinator::route::resolve::Refusal;
use oss_rest::{Exception, JSON, Request, Response, RestError};

/// The content type of a text table.
const TEXT: &str = "text/plain; charset=UTF-8";

/// A coordinator refusal as the error the edge answers with.
pub(crate) fn refused(r: Refusal) -> RestError {
    RestError::exception(r.status, Exception::new(r.kind, r.reason))
}

/// An error of this status, type and reason.
pub(crate) fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

/// A boolean parameter as `Booleans.parseBoolean` reads it: absent is
/// `default`, and anything but `false`, `0`, `off` and `no` is true, so a
/// bare `?v` is true.
pub(crate) fn flag(req: &Request<'_>, name: &str, default: bool) -> bool {
    match req.param(name) {
        None => default,
        Some(v) => !matches!(&*v, "false" | "0" | "off" | "no"),
    }
}

/// Sends a drawn table: JSON when it is structured, else text.
pub(crate) fn send_table(resp: &mut Response<'_>, answer: &Answer) -> Result<(), RestError> {
    let kind = if answer.structured { JSON } else { TEXT };
    resp.send(200, kind, answer.body.as_bytes())
}
