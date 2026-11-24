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

use crate::error::RestError;
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
    #[cfg_attr(not(test), allow(dead_code))]
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
    #[cfg(test)]
    pub(crate) fn pattern(&self) -> &'static str {
        self.pattern
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
mod tests {
    use super::*;

    fn ok(_: &mut Request<'_>, _: &mut Response<'_>) -> Result<(), RestError> {
        Ok(())
    }

    fn segs(path: &str) -> Vec<&str> {
        split(path).map(|r| &path[r]).collect()
    }

    #[test]
    fn paths_split_like_java() {
        assert_eq!(segs("/"), Vec::<&str>::new());
        assert_eq!(segs(""), Vec::<&str>::new());
        assert_eq!(segs("/_search"), ["_search"]);
        assert_eq!(segs("/_search/"), ["_search"]);
        assert_eq!(segs("/a//b"), ["a", "", "b"]);
        assert_eq!(segs("//a"), ["", "a"]);
        assert_eq!(segs("a/b"), ["a", "b"]);
        assert_eq!(segs("/a/b///"), ["a", "b"]);
    }

    fn router() -> Router {
        Router::new()
            .route(Method::Get, "/_search", &["q"], ok)
            .and_then(|r| r.route(Method::Post, "/_search", &["q"], ok))
            .and_then(|r| r.route(Method::Get, "/{index}/_search", &["q", "size"], ok))
            .and_then(|r| r.route(Method::Get, "/{index}/{type}", &[], ok))
            .and_then(|r| r.route(Method::Get, "/_cat/{what}", &[], ok))
            .and_then(|r| r.route(Method::Put, "/{index}", &[], ok))
            .and_then(|r| r.route(Method::Get, "/", &[], ok))
            .expect("routes register")
    }

    fn found(
        r: &Router,
        m: Method,
        path: &str,
    ) -> Option<(&'static str, Vec<(&'static str, String)>)> {
        let mut bound = Vec::new();
        match r.resolve(Some(m), path, &mut bound) {
            Resolved::Found(route) => Some((
                route.pattern(),
                bound
                    .iter()
                    .map(|(n, rg)| (*n, path[rg.clone()].to_owned()))
                    .collect(),
            )),
            _ => None,
        }
    }

    #[test]
    fn explicit_segments_beat_patterns() {
        let r = router();
        assert_eq!(
            found(&r, Method::Get, "/_search").map(|f| f.0),
            Some("/_search")
        );
        assert_eq!(
            found(&r, Method::Get, "/logs/_search"),
            Some(("/{index}/_search", vec![("index", "logs".to_owned())]))
        );
        assert_eq!(
            found(&r, Method::Get, "/_cat/indices").map(|f| f.0),
            Some("/_cat/{what}")
        );
        assert_eq!(
            found(&r, Method::Get, "/logs/x"),
            Some((
                "/{index}/{type}",
                vec![("index", "logs".to_owned()), ("type", "x".to_owned())]
            ))
        );
        assert_eq!(found(&r, Method::Get, "/").map(|f| f.0), Some("/"));
        assert_eq!(
            found(&r, Method::Get, "/logs/_search/").map(|f| f.0),
            Some("/{index}/_search")
        );
    }

    #[test]
    fn other_methods_and_no_match() {
        let r = router();
        let mut bound = Vec::new();
        // `/{index}` matches `/_search` too, so PUT is allowed, as in Java.
        match r.resolve(Some(Method::Delete), "/_search", &mut bound) {
            Resolved::OtherMethods(m) => assert_eq!(m.to_string(), "[GET, POST, PUT]"),
            _ => panic!("expected other methods"),
        }
        match r.resolve(None, "/_search", &mut bound) {
            Resolved::OtherMethods(m) => assert_eq!(m.to_string(), "[GET, POST, PUT]"),
            _ => panic!("expected other methods"),
        }
        assert!(matches!(
            r.resolve(Some(Method::Get), "/a/b/c/d", &mut bound),
            Resolved::NotFound
        ));
        // `/logs` matches the PUT route only.
        match r.resolve(Some(Method::Get), "/logs", &mut bound) {
            Resolved::OtherMethods(m) => assert_eq!(m.to_string(), "[PUT]"),
            _ => panic!("expected other methods"),
        }
    }

    #[test]
    fn accepted_parameters_include_path_parameters() {
        let r = router();
        let mut bound = Vec::new();
        let Resolved::Found(route) = r.resolve(Some(Method::Get), "/i/_search", &mut bound) else {
            panic!("found");
        };
        assert!(route.accepts("size") && route.accepts("index") && !route.accepts("from"));
        assert_eq!(
            route.candidates().collect::<Vec<_>>(),
            ["q", "size", "index"]
        );
    }

    #[test]
    fn bad_and_duplicate_patterns_are_refused() {
        let dup = Router::new()
            .route(Method::Get, "/{a}/x", &[], ok)
            .and_then(|r| r.route(Method::Get, "/{b}/x", &[], ok));
        assert_eq!(
            dup.err(),
            Some(RouteError::Duplicate {
                method: Method::Get,
                pattern: "/{b}/x"
            })
        );
        assert!(
            Router::new()
                .route(Method::Get, "/{a}/x", &[], ok)
                .and_then(|r| r.route(Method::Post, "/{b}/x", &[], ok))
                .is_ok()
        );
        for bad in ["/{}", "/a{b}", "/{a", "/{{a}}"] {
            assert_eq!(
                Router::new().route(Method::Get, bad, &[], ok).err(),
                Some(RouteError::BadPattern(bad))
            );
        }
    }

    #[test]
    fn the_segment_limit_holds_at_max_and_refuses_one_more() {
        let at = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p";
        let over = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q";
        assert!(Router::new().route(Method::Get, at, &[], ok).is_ok());
        assert_eq!(
            Router::new().route(Method::Get, over, &[], ok).err(),
            Some(RouteError::TooManySegments(over))
        );
    }

    #[test]
    fn the_first_position_that_differs_decides_which_route_is_more_explicit() {
        let r = Router::new()
            .route(Method::Get, "/{a}/lit", &[], ok)
            .and_then(|r| r.route(Method::Get, "/lit/{b}", &[], ok))
            .expect("routes register");
        let (param_first, literal_first) = (&r.routes[0], &r.routes[1]);
        assert!(more_explicit(literal_first, param_first));
        assert!(!more_explicit(param_first, literal_first));
        assert!(!more_explicit(param_first, param_first));
    }
}
