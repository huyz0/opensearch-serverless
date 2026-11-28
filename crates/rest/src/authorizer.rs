//! A check a request passes once its route is found and before anything of
//! it is read: how a node with authorization refuses what a user may not do.

use std::borrow::Cow;

use crate::guard::{Denial, Identity};
use crate::method::Method;
use crate::query::decode_component;
use crate::router::PathParam;

/// What an authorizer sees of a request that found its route.
#[derive(Debug, Clone, Copy)]
pub struct Authorization<'a> {
    pub(crate) identity: Option<&'a Identity>,
    pub(crate) method: Option<Method>,
    pub(crate) pattern: &'static str,
    pub(crate) prefix: bool,
    pub(crate) path: &'a str,
    pub(crate) params: &'a [PathParam],
}

impl<'a> Authorization<'a> {
    /// Who the request acts as, as the guard said; none when no guard ran.
    pub fn principal(&self) -> Option<&'a str> {
        self.identity.map(Identity::name)
    }

    /// Who the request acts as, with the roles a token carried.
    pub fn identity(&self) -> Option<&'a Identity> {
        self.identity
    }

    /// The method, or `None` for one this server does not know.
    pub fn method(&self) -> Option<Method> {
        self.method
    }

    /// The route's pattern as registered; a prefix route's prefix.
    pub fn pattern(&self) -> &'static str {
        self.pattern
    }

    /// Whether the route is a prefix route, which takes any method and path
    /// under its prefix.
    pub fn is_prefix(&self) -> bool {
        self.prefix
    }

    /// The path parameter `name`, percent-decoded as the handler will read
    /// it: `None` when the route has no such parameter, and `Some(None)`
    /// when its bytes do not decode.
    pub fn path_param(&self, name: &str) -> Option<Option<Cow<'a, str>>> {
        self.params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, r)| decode_component(&self.path[r.clone()], false).ok())
    }
}

/// Decides, for each request that found a route, whether it may run.
pub trait Authorizer: Send + Sync {
    /// `None` lets the request run; a [`Denial`] is sent in its place.
    fn authorize(&self, req: &Authorization<'_>) -> Option<Denial>;
}
