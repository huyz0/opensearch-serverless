//! Routes: a method, a path pattern and the query parameters a handler takes.
//!
//! Matching follows `RestController#tryAllHandlers` and
//! `PathTrie#retrieveAll` (server/src/main/java/org/opensearch/common/path/PathTrie.java)
//! at huyz0/OpenSearch@49f238ed:
//!
//! - a path splits on `/` like Java's `String#split`: the leading empty
//!   segment and trailing empty ones are dropped, inner empty ones kept;
//! - `{name}` matches any one segment and binds it, percent-decoded, as a
//!   parameter; an explicit segment beats a pattern at the same position, and
//!   an earlier explicit segment beats a later one;
//! - a path that matches routes of other methods only is answered 405 with
//!   `Allow`, `OPTIONS` on it 200 with `Allow`; a path nothing matches, 400.
//!
//! The table is a list scanned in full, not a trie: a few hundred routes of a
//! few segments each cost microseconds, and the scan allocates nothing. A
//! trie is the change to make if a profile ever shows it.

use std::sync::Arc;

use crate::admission::SearchAdmission;
use crate::authorizer::Authorizer;
use crate::error::RestError;
use crate::guard::Guard;
use crate::method::{Method, MethodSet};
use crate::request::Request;
use crate::response::Response;

/// Serves one route.
pub trait Handler: Send + Sync {
    /// Handles `req`, writing the answer to `resp`. An error returned before
    /// the response starts is sent as its error body; after, the connection
    /// is closed, since the status line is already out.
    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError>;
}

impl<F> Handler for F
where
    F: Fn(&mut Request<'_>, &mut Response<'_>) -> Result<(), RestError> + Send + Sync,
{
    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        self(req, resp)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(&'static str),
    Param(&'static str),
}

/// One registered route.
pub(crate) struct Route {
    method: Method,
    pattern: &'static str,
    segments: Vec<Segment>,
    params: &'static [&'static str],
    handler: Arc<dyn Handler>,
    /// A prefix route: any method, any path under its segments, any parameter.
    prefix: bool,
}

impl Route {
    /// The query parameters the route takes, and its path parameters; a
    /// prefix route takes every one, its handler being the judge.
    pub(crate) fn accepts(&self, name: &str) -> bool {
        self.prefix || self.params.contains(&name) || self.path_param_names().any(|p| p == name)
    }

    /// The declared and path parameter names, the "did you mean" candidates.
    pub(crate) fn candidates(&self) -> impl Iterator<Item = &'static str> + Clone + '_ {
        self.params.iter().copied().chain(self.path_param_names())
    }

    fn path_param_names(&self) -> impl Iterator<Item = &'static str> + Clone + '_ {
        self.segments.iter().filter_map(|s| match s {
            Segment::Param(p) => Some(*p),
            Segment::Literal(_) => None,
        })
    }

    pub(crate) fn handler(&self) -> &dyn Handler {
        self.handler.as_ref()
    }

    /// The pattern as registered.
    pub(crate) fn pattern(&self) -> &'static str {
        self.pattern
    }

    /// Whether this is a prefix route.
    pub(crate) fn is_prefix(&self) -> bool {
        self.prefix
    }
}

/// A route that could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// The same method and path shape is already registered.
    Duplicate {
        /// The method.
        method: Method,
        /// The pattern registered second.
        pattern: &'static str,
    },
    /// A segment holds `{` or `}` without being one whole `{name}`.
    BadPattern(&'static str),
    /// More segments than [`MAX_SEGMENTS`].
    TooManySegments(&'static str),
}

/// The most segments a pattern may have.
pub const MAX_SEGMENTS: usize = 16;

/// The route table, built once at startup and shared by every worker.
#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
    /// Prefix routes, tried only when no ordinary route has the path and method.
    prefixes: Vec<Route>,
    guard: Option<Arc<dyn Guard>>,
    authorizer: Option<Arc<dyn Authorizer>>,
    admission: Option<Arc<dyn SearchAdmission>>,
}

/// Where a path and method lead.
pub(crate) enum Resolved<'r> {
    /// A route; its path parameters are in the caller's list.
    Found(&'r Route),
    /// Routes match the path with other methods only.
    OtherMethods(MethodSet),
    /// Nothing matches the path.
    NotFound,
}

/// A bound path parameter: the route's name and the raw segment's range in
/// the path.
pub(crate) type PathParam = (&'static str, std::ops::Range<usize>);

impl Router {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts `guard` in front of every request this router serves.
    #[must_use]
    pub fn with_guard(mut self, guard: Arc<dyn Guard>) -> Self {
        self.guard = Some(guard);
        self
    }

    /// Puts `authorizer` between finding a route and running it.
    #[must_use]
    pub fn with_authorizer(mut self, authorizer: Arc<dyn Authorizer>) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    /// Lets `admission` park a request until its tenant's turn, when the
    /// server's `search_queue_size` is above 0.
    pub fn with_search_admission(mut self, admission: Arc<dyn SearchAdmission>) -> Self {
        self.admission = Some(admission);
        self
    }

    /// The search admission hook, if one is set.
    pub(crate) fn search_admission(&self) -> Option<Arc<dyn SearchAdmission>> {
        self.admission.clone()
    }

    /// Whether an authorizer is set.
    pub fn has_authorizer(&self) -> bool {
        self.authorizer.is_some()
    }

    /// The authorizer, if one is set.
    pub(crate) fn authorizer(&self) -> Option<&dyn Authorizer> {
        self.authorizer.as_deref()
    }

    /// Every registered route: its method (none for a prefix route, which
    /// takes any) and pattern.
    pub fn routes(&self) -> Vec<(Option<Method>, &'static str)> {
        self.routes
            .iter()
            .map(|r| (Some(r.method), r.pattern))
            // ALLOC: a test or startup listing, one entry a route.
            .chain(self.prefixes.iter().map(|r| (None, r.pattern)))
            .collect()
    }

    /// Whether a guard is set.
    pub fn has_guard(&self) -> bool {
        self.guard.is_some()
    }

    /// The guard, if one is set.
    pub(crate) fn guard(&self) -> Option<&dyn Guard> {
        self.guard.as_deref()
    }

    /// Registers `handler` for `method` on `pattern` (such as
    /// `/{index}/_search`), taking the query parameters in `params`.
    pub fn route(
        mut self,
        method: Method,
        pattern: &'static str,
        params: &'static [&'static str],
        handler: impl Handler + 'static,
    ) -> Result<Self, RouteError> {
        // ALLOC: startup only; one entry per segment, at most MAX_SEGMENTS.
        let mut segments = Vec::new();
        for s in split(pattern) {
            let s = &pattern[s];
            let seg = match s.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
                Some(name) if !name.is_empty() && !name.contains(['{', '}']) => {
                    Segment::Param(name)
                }
                _ if s.contains(['{', '}']) => return Err(RouteError::BadPattern(pattern)),
                _ => Segment::Literal(s),
            };
            if segments.len() == MAX_SEGMENTS {
                return Err(RouteError::TooManySegments(pattern));
            }
            segments.push(seg);
        }
        let same_shape = |r: &Route| {
            r.method == method
                && r.segments.len() == segments.len()
                && r.segments.iter().zip(&segments).all(|(a, b)| match (a, b) {
                    (Segment::Literal(x), Segment::Literal(y)) => x == y,
                    (Segment::Param(_), Segment::Param(_)) => true,
                    _ => false,
                })
        };
        if self.routes.iter().any(same_shape) {
            return Err(RouteError::Duplicate { method, pattern });
        }
        self.routes.push(Route {
            method,
            pattern,
            segments,
            params,
            // ALLOC: startup only; one handler per route.
            handler: Arc::new(handler),
            prefix: false,
        });
        Ok(self)
    }

    /// Registers `handler` for every method on `prefix` and every path under
    /// it, matched on whole segments (`/_plugins/_ml` takes `/_plugins/_ml/x`
    /// and not `/_plugins/_mlx`). It is tried only when no ordinary route has
    /// the path and method, so a route registered with [`Router::route`]
    /// always wins; and it takes any query parameter.
    pub fn prefix(
        mut self,
        prefix: &'static str,
        handler: impl Handler + 'static,
    ) -> Result<Self, RouteError> {
        // ALLOC: startup only; one entry per segment, at most MAX_SEGMENTS.
        let mut segments = Vec::new();
        for s in split(prefix) {
            let s = &prefix[s];
            if s.contains(['{', '}']) {
                return Err(RouteError::BadPattern(prefix));
            }
            if segments.len() == MAX_SEGMENTS {
                return Err(RouteError::TooManySegments(prefix));
            }
            segments.push(Segment::Literal(s));
        }
        if segments.is_empty() || self.prefixes.iter().any(|r| r.pattern == prefix) {
            return Err(RouteError::BadPattern(prefix));
        }
        self.prefixes.push(Route {
            method: Method::Get,
            pattern: prefix,
            segments,
            params: &[],
            // ALLOC: startup only; one handler per prefix.
            handler: Arc::new(handler),
            prefix: true,
        });
        Ok(self)
    }

    /// Resolves `method` on the raw `path`, binding path parameters into
    /// `bound` (cleared first; it never holds more than [`MAX_SEGMENTS`]).
    pub(crate) fn resolve<'r>(
        &'r self,
        method: Option<Method>,
        path: &str,
        bound: &mut Vec<PathParam>,
    ) -> Resolved<'r> {
        let mut best: Option<&Route> = None;
        let mut methods = MethodSet::EMPTY;
        for route in &self.routes {
            if !matches(route, path) {
                continue;
            }
            methods.insert(route.method);
            if Some(route.method) == method && best.is_none_or(|b| more_explicit(route, b)) {
                best = Some(route);
            }
        }
        bound.clear();
        if best.is_none()
            && method.is_some()
            && let Some(route) = self.prefixes.iter().find(|r| under(r, path))
        {
            return Resolved::Found(route);
        }
        match best {
            Some(route) => {
                for (seg, range) in route.segments.iter().zip(split(path)) {
                    if let Segment::Param(name) = seg {
                        bound.push((name, range));
                    }
                }
                Resolved::Found(route)
            }
            None if methods.is_empty() => Resolved::NotFound,
            None => Resolved::OtherMethods(methods),
        }
    }
}

/// Whether `a` is preferred over `b`: at the first position where one has a
/// literal and the other a parameter, the literal wins.
fn more_explicit(a: &Route, b: &Route) -> bool {
    for (x, y) in a.segments.iter().zip(&b.segments) {
        match (x, y) {
            (Segment::Literal(_), Segment::Param(_)) => return true,
            (Segment::Param(_), Segment::Literal(_)) => return false,
            _ => {}
        }
    }
    false
}

/// Whether `path` is the prefix route's path or under it, on whole segments.
fn under(route: &Route, path: &str) -> bool {
    let mut segs = split(path);
    route.segments.iter().all(|seg| match (seg, segs.next()) {
        (Segment::Literal(lit), Some(r)) => path[r] == **lit,
        _ => false,
    })
}

fn matches(route: &Route, path: &str) -> bool {
    let mut segs = split(path);
    for seg in &route.segments {
        let Some(r) = segs.next() else {
            return false;
        };
        if let Segment::Literal(lit) = seg
            && path[r] != **lit
        {
            return false;
        }
    }
    segs.next().is_none()
}

/// The segments of `path` as ranges, split as Java's `String#split("/")`
/// splits after `PathTrie` skips the leading empty one.
pub(crate) fn split(path: &str) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let trimmed = path.trim_end_matches('/');
    let (start, body) = match trimmed.strip_prefix('/') {
        Some(rest) => (1usize, rest),
        None => (0, trimmed),
    };
    let mut pos = start;
    let mut done = body.is_empty();
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let rest = &path[pos..start.saturating_add(body.len())];
        let len = rest.find('/').unwrap_or_else(|| {
            done = true;
            rest.len()
        });
        let r = pos..pos.saturating_add(len);
        pos = r.end.saturating_add(1);
        Some(r)
    })
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
