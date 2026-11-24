//! A routed request, as a handler sees it.
//!
//! The path, query string and headers borrow the worker's head buffer, so
//! reading them allocates nothing unless a value carries percent-escapes.
//! The body streams; see [`Body`].

use std::borrow::Cow;

use crate::body::Body;
use crate::head::Head;
use crate::method::Method;
use crate::query::{QueryParams, decode_component};
use crate::router::PathParam;

/// One request.
#[derive(Debug)]
pub struct Request<'a> {
    pub(crate) method: Method,
    pub(crate) path: &'a str,
    pub(crate) query: &'a str,
    pub(crate) params: &'a QueryParams,
    pub(crate) path_params: &'a [PathParam],
    pub(crate) head: &'a Head,
    pub(crate) head_buf: &'a [u8],
    pub(crate) body: Body<'a>,
}

impl<'a> Request<'a> {
    /// The method.
    pub fn method(&self) -> Method {
        self.method
    }

    /// The raw path, before percent-decoding, without the query string.
    pub fn raw_path(&self) -> &'a str {
        self.path
    }

    /// A path parameter bound by the route's `{name}`, percent-decoded.
    /// Decoding was checked when the request was routed.
    pub fn path_param(&self, name: &str) -> Option<Cow<'a, str>> {
        let path = self.path;
        self.path_params
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, r)| decode_component(&path[r.clone()], false).ok())
    }

    /// A query parameter, percent-decoded; the last one wins when repeated.
    pub fn param(&self, name: &str) -> Option<Cow<'a, str>> {
        self.params.get(self.query, name)
    }

    /// The first value of a header, compared case-insensitively.
    pub fn header(&self, name: &str) -> Option<&'a str> {
        self.head.header(self.head_buf, name)
    }

    /// Every header as a name and value, in the order sent.
    pub fn headers(&self) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.head.all(self.head_buf)
    }

    /// The raw query string, without the `?`.
    pub fn raw_query(&self) -> &'a str {
        self.query
    }

    /// The body.
    pub fn body(&mut self) -> &mut Body<'a> {
        &mut self.body
    }
}
