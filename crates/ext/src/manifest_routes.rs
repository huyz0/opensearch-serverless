//! A manifest's `routes` section: each route's method, path, action and the
//! query parameters it takes, every length and count bounded before it is
//! kept (docs/specs/plugin-system AC-5).

use crate::descriptor::{DescriptorError, Members, path};
use crate::json::Value;
use crate::manifest::{ManifestPoint, Route, array, serves};

const MAX_ROUTES: usize = 64;
const MAX_QUERY: usize = 16;
const MAX_QUERY_NAME: usize = 40;
const MAX_PATH_BYTES: usize = 256;
const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "HEAD", "PATCH"];

pub(crate) fn routes(
    top: &Members,
    points: &[ManifestPoint],
) -> Result<Vec<Route>, DescriptorError> {
    let Some(items) = array(top, "routes")? else {
        return Ok(Vec::new());
    };
    if !serves(points, "rest") {
        return Err(DescriptorError::at("routes", "needs the rest point"));
    }
    if items.len() > MAX_ROUTES {
        return Err(DescriptorError::at("routes", "more than 64 routes"));
    }
    // ALLOC: one manifest, at most MAX_ROUTES entries.
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        // ALLOC: one manifest, a short key per route.
        let at = format!("routes[{i}]");
        let Value::Obj(fields) = item else {
            return Err(DescriptorError::at(&at, "not an object"));
        };
        // ALLOC: one manifest, three members.
        let one = Members::new(&at, fields.clone(), &["method", "path", "action", "query"])?;
        let method = one.string("method")?;
        if !METHODS.contains(&method.as_str()) {
            return Err(DescriptorError::at(&path(&at, "method"), "not a method"));
        }
        let route = one.string("path")?;
        if !route_path(&route) {
            return Err(DescriptorError::at(
                &path(&at, "path"),
                "not a valid route path",
            ));
        }
        let action = one.string("action")?;
        let named = (1..=64).contains(&action.len())
            && action
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_:/.-".contains(c));
        if !named {
            return Err(DescriptorError::at(
                &path(&at, "action"),
                "not a valid action name",
            ));
        }
        let query = query_names(&one, &at)?;
        out.push(Route {
            method,
            path: route,
            action,
            query,
        });
    }
    Ok(out)
}

/// The route's optional `query` list: names of at most 16 parameters, each
/// lower-case letters, digits and underscores, starting with a letter.
fn query_names(one: &Members, at: &str) -> Result<Vec<String>, DescriptorError> {
    let key = path(at, "query");
    let Some(items) = array(one, "query")? else {
        return Ok(Vec::new());
    };
    if items.len() > MAX_QUERY {
        return Err(DescriptorError::at(&key, "more than 16 parameters"));
    }
    // ALLOC: one route, at most MAX_QUERY names.
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let Value::Str(name) = item else {
            return Err(DescriptorError::at(&key, "not a string"));
        };
        let mut chars = name.chars();
        let ok = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && name.len() <= MAX_QUERY_NAME
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok || out.contains(name) {
            return Err(DescriptorError::at(
                &key,
                "not a parameter name, or repeated",
            ));
        }
        out.push(name.clone());
    }
    Ok(out)
}

/// `/`, or `/` then segments joined by `/`: each a literal of `[A-Za-z0-9_.~-]`
/// (not `.` or `..`) or one `{name}` parameter.
fn route_path(text: &str) -> bool {
    if text.len() > MAX_PATH_BYTES || !text.starts_with('/') {
        return false;
    }
    if text == "/" {
        return true;
    }
    text.get(1..).unwrap_or_default().split('/').all(segment)
}

fn segment(seg: &str) -> bool {
    if let Some(name) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut chars = name.chars();
        return chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    }
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.~-".contains(c))
}
