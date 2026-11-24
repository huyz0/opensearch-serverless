//! A check every request passes before its route is looked up: how a node
//! with authentication refuses what carries no valid credentials, even for a
//! path nothing serves.

use std::sync::Arc;

use crate::method::Method;

/// What a guard sees of a request: its method, path and credentials.
#[derive(Debug, Clone, Copy)]
pub struct Admission<'a> {
    pub(crate) method: Option<Method>,
    pub(crate) path: &'a str,
    pub(crate) authorization: Option<&'a str>,
}

impl<'a> Admission<'a> {
    /// The method, or `None` for one this server does not know.
    pub fn method(&self) -> Option<Method> {
        self.method
    }

    /// The raw path, without the query.
    pub fn path(&self) -> &'a str {
        self.path
    }

    /// The `Authorization` header, if any.
    pub fn authorization(&self) -> Option<&'a str> {
        self.authorization
    }
}

/// A refusal: sent as it is, in place of the route's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The HTTP status.
    pub status: u16,
    /// The `Content-Type` of the body.
    pub content_type: &'static str,
    /// The body.
    pub body: String,
    /// A `WWW-Authenticate` challenge to send with it.
    pub challenge: Option<&'static str>,
}

/// What a guard decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Serve the request; the name, when there is one, is who it acts as.
    Allow(Option<Arc<str>>),
    /// Refuse it.
    Deny(Denial),
}

/// Admits or refuses every request, whether or not a route matches it.
pub trait Guard: Send + Sync {
    /// Decides for one request.
    fn admit(&self, req: &Admission<'_>) -> Verdict;
}
