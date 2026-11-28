//! The plugin manifest: what a plugin says about itself, parsed by one strict
//! parser whatever tier delivers it
//! ([ADR 0080](../../../docs/adr/0080-one-plugin-system-three-tiers.md),
//! section 2).
//!
//! Exact keys, no duplicates, every value checked before it is kept, every
//! refusal naming the key. A caller's tier is judged by [`Manifest::for_tier`]
//! after the parse, so the same bytes are refused for the same reason by every
//! loader.

use crate::MAX_DESCRIPTOR_BYTES;
use crate::capability::{Budget, Capability, MAX_BUDGET};
use crate::catalogue::{Determinism, Tier, point};
use crate::descriptor::{DescriptorError, Limits, Members, limits, pair, path, triple};
use crate::json::{self, Fail, Value};
use crate::version::{VersionError, abi_accepted_against, pack_abi};

/// The plugin API this host speaks, `(major, minor)`.
pub const PLUGIN_API: (u16, u16) = (1, 0);

const MAX_POINTS: usize = 16;
const MAX_ROUTES: usize = 64;
const MAX_CAPABILITIES: usize = 16;
const MAX_PATH_BYTES: usize = 256;
const EVENTS: &[&str] = &[
    "node.started",
    "node.stopping",
    "index.created",
    "index.deleted",
];
const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "HEAD", "PATCH"];

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// Built for a plugin API this host does not accept; judged before any
    /// other key, so a newer manifest is refused for that.
    Api(VersionError),
    /// A key is at fault.
    Invalid(DescriptorError),
}

impl From<DescriptorError> for ManifestError {
    fn from(e: DescriptorError) -> Self {
        Self::Invalid(e)
    }
}

/// A point a plugin serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestPoint {
    name: String,
    version: u32,
}

impl ManifestPoint {
    /// The point's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The point's version.
    pub fn version(&self) -> u32 {
        self.version
    }
}

/// A REST route a plugin declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    method: String,
    path: String,
    action: String,
}

impl Route {
    /// The HTTP method.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The path, with `{name}` segments.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The action name the route asks to be authorised as.
    pub fn action(&self) -> &str {
        &self.action
    }
}

/// A plugin's manifest, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    id: String,
    version: (u32, u32, u32),
    api: (u16, u16),
    points: Vec<ManifestPoint>,
    routes: Vec<Route>,
    events: Vec<String>,
    capabilities: Vec<Capability>,
    budget: Budget,
    limits: Limits,
}

impl Manifest {
    /// Parses and validates `bytes`. The plugin API version is judged first.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Api`] for a version this host does not accept, else
    /// [`ManifestError::Invalid`] naming the first key at fault, or none for
    /// a document that is too large, not UTF-8 or not JSON.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES as usize {
            return Err(DescriptorError::at("", "larger than the manifest ceiling").into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| DescriptorError::at("", "not UTF-8"))?;
        let root = match json::parse(text) {
            Ok(v) => v,
            Err(Fail::Duplicate(k)) => return Err(DescriptorError::at(&k, "repeated").into()),
            Err(Fail::Syntax) => return Err(DescriptorError::at("", "not JSON").into()),
        };
        let Value::Obj(items) = root else {
            return Err(DescriptorError::at("", "not an object").into());
        };
        if let Some((major, minor)) = declared_api(&items) {
            abi_accepted_against(PLUGIN_API.0, PLUGIN_API.1, pack_abi(major, minor))
                .map_err(ManifestError::Api)?;
        }
        Self::read(items).map_err(Into::into)
    }

    fn read(items: Vec<(String, Value)>) -> Result<Self, DescriptorError> {
        let top = Members::new(
            "",
            items,
            &[
                "plugin",
                "points",
                "routes",
                "events",
                "capabilities",
                "budget",
                "limits",
            ],
        )?;
        let head = object(&top, "plugin", &["id", "version", "api"])?;
        let id = id(&head.string("id")?)?;
        let version = triple(&head.string("version")?, "plugin.version")?;
        let api = pair(&head.string("api")?, "plugin.api")?;
        let points = points(&top)?;
        let routes = routes(&top, &points)?;
        let events = events(&top, &points)?;
        Ok(Self {
            id,
            version,
            api,
            points,
            routes,
            events,
            capabilities: capabilities(&top)?,
            budget: budget(&top)?,
            limits: limits(&top)?,
        })
    }

    /// Whether this manifest may run on `tier`: every point is one the
    /// catalogue lets that tier serve, a pure point holds no capability, and
    /// an out-of-process plugin holds only what a process can be given.
    ///
    /// # Errors
    ///
    /// [`DescriptorError`] naming the first point or capability at fault.
    pub fn for_tier(&self, tier: Tier) -> Result<(), DescriptorError> {
        for (i, p) in self.points.iter().enumerate() {
            let spec = point(&p.name).ok_or_else(|| {
                DescriptorError::at(&format!("points[{i}].name"), "not in the catalogue")
            })?;
            if !spec.served_on(tier) {
                return Err(DescriptorError::at(
                    &format!("points[{i}].name"),
                    match tier {
                        Tier::Native => "not served on the native tier",
                        Tier::Wasm => "not served on the wasm tier",
                        Tier::Grpc => "not served on the grpc tier",
                    },
                ));
            }
            if spec.determinism == Determinism::Pure && !self.capabilities.is_empty() {
                return Err(DescriptorError::at(
                    "capabilities",
                    "a pure point takes no capability",
                ));
            }
        }
        for (i, c) in self.capabilities.iter().enumerate() {
            if !c.allowed_on(tier) {
                return Err(DescriptorError::at(
                    &format!("capabilities[{i}]"),
                    "not given to this tier",
                ));
            }
        }
        Ok(())
    }

    /// The plugin's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The plugin's own version.
    pub fn version(&self) -> (u32, u32, u32) {
        self.version
    }

    /// The plugin API `(major, minor)` it was built against.
    pub fn api(&self) -> (u16, u16) {
        self.api
    }

    /// The points it serves.
    pub fn points(&self) -> &[ManifestPoint] {
        &self.points
    }

    /// The routes it declares.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// The events it subscribes to.
    pub fn events(&self) -> &[String] {
        &self.events
    }

    /// The capabilities it asks for.
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// The most each call may do.
    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// What it asks to lower.
    pub fn limits(&self) -> Limits {
        self.limits
    }
}

/// The `plugin.api` a manifest declares, when it declares one this reads.
fn declared_api(items: &[(String, Value)]) -> Option<(u16, u16)> {
    let Value::Obj(head) = &items.iter().find(|(k, _)| k == "plugin")?.1 else {
        return None;
    };
    let Value::Str(api) = &head.iter().find(|(k, _)| k == "api")?.1 else {
        return None;
    };
    pair(api, "plugin.api").ok()
}

/// The members of the object at `key`, which must be there.
fn object(top: &Members, key: &str, allowed: &[&str]) -> Result<Members, DescriptorError> {
    let Value::Obj(items) = top.require(key)? else {
        return Err(DescriptorError::at(key, "not an object"));
    };
    // ALLOC: one manifest, at most a handful of members.
    Members::new(key, items.clone(), allowed)
}

/// An id of 2 to 41 characters: lowercase to start, then lowercase, digits, `_` and `-`.
fn id(text: &str) -> Result<String, DescriptorError> {
    let mut chars = text.chars();
    let ok = (2..=41).contains(&text.len())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        // ALLOC: one manifest, at most 41 bytes.
        Ok(text.to_owned())
    } else {
        Err(DescriptorError::at("plugin.id", "not a valid plugin id"))
    }
}

fn array<'a>(top: &'a Members, key: &str) -> Result<Option<&'a [Value]>, DescriptorError> {
    match top.get(key) {
        None => Ok(None),
        Some(Value::Arr(items)) => Ok(Some(items)),
        Some(_) => Err(DescriptorError::at(key, "not an array")),
    }
}

fn points(top: &Members) -> Result<Vec<ManifestPoint>, DescriptorError> {
    let Value::Arr(items) = top.require("points")? else {
        return Err(DescriptorError::at("points", "not an array"));
    };
    if items.is_empty() || items.len() > MAX_POINTS {
        return Err(DescriptorError::at("points", "needs 1 to 16 points"));
    }
    // ALLOC: one manifest, at most MAX_POINTS entries.
    let mut out: Vec<ManifestPoint> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        // ALLOC: one manifest, a short key per point.
        let at = format!("points[{i}]");
        let Value::Obj(fields) = item else {
            return Err(DescriptorError::at(&at, "not an object"));
        };
        // ALLOC: one manifest, two members.
        let one = Members::new(&at, fields.clone(), &["name", "version"])?;
        let name = one.string("name")?;
        let key = path(&at, "name");
        let spec = point(&name).ok_or_else(|| DescriptorError::at(&key, "not in the catalogue"))?;
        if out.iter().any(|p| p.name == name) {
            return Err(DescriptorError::at(&key, "repeated"));
        }
        let version = one.int("version", 0, u64::from(u32::MAX))?;
        if version != u64::from(spec.version) {
            return Err(DescriptorError::at(
                &path(&at, "version"),
                "not the version this host serves",
            ));
        }
        out.push(ManifestPoint {
            name,
            version: spec.version,
        });
    }
    Ok(out)
}

fn serves(points: &[ManifestPoint], name: &str) -> bool {
    points.iter().any(|p| p.name == name)
}

fn routes(top: &Members, points: &[ManifestPoint]) -> Result<Vec<Route>, DescriptorError> {
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
        let one = Members::new(&at, fields.clone(), &["method", "path", "action"])?;
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
        out.push(Route {
            method,
            path: route,
            action,
        });
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

fn events(top: &Members, points: &[ManifestPoint]) -> Result<Vec<String>, DescriptorError> {
    let Some(items) = array(top, "events")? else {
        return Ok(Vec::new());
    };
    if !serves(points, "lifecycle") {
        return Err(DescriptorError::at("events", "needs the lifecycle point"));
    }
    // ALLOC: one manifest, at most one entry per known event.
    let mut out: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let at = format!("events[{i}]");
        match item {
            Value::Str(s) if EVENTS.contains(&s.as_str()) && !out.contains(s) => {
                out.push(s.clone());
            }
            _ => return Err(DescriptorError::at(&at, "not a known event, or repeated")),
        }
    }
    Ok(out)
}

fn capabilities(top: &Members) -> Result<Vec<Capability>, DescriptorError> {
    let Some(items) = array(top, "capabilities")? else {
        return Ok(Vec::new());
    };
    if items.len() > MAX_CAPABILITIES {
        return Err(DescriptorError::at(
            "capabilities",
            "more than 16 capabilities",
        ));
    }
    // ALLOC: one manifest, at most MAX_CAPABILITIES entries.
    let mut out: Vec<Capability> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let at = format!("capabilities[{i}]");
        let cap = match item {
            Value::Str(s) => Capability::parse(s),
            _ => None,
        };
        match cap {
            Some(c) if !out.contains(&c) => out.push(c),
            _ => {
                return Err(DescriptorError::at(
                    &at,
                    "not a known capability, or repeated",
                ));
            }
        }
    }
    Ok(out)
}

fn budget(top: &Members) -> Result<Budget, DescriptorError> {
    let Some(value) = top.get("budget") else {
        return Ok(Budget::default());
    };
    let Value::Obj(items) = value else {
        return Err(DescriptorError::at("budget", "not an object"));
    };
    // ALLOC: one manifest, at most three members.
    let inner = Members::new(
        "budget",
        items.clone(),
        &["state_ops", "searches", "egress"],
    )?;
    let one = |key: &str| -> Result<u32, DescriptorError> {
        if inner.get(key).is_none() {
            return Ok(0);
        }
        let n = inner.int(key, 0, MAX_BUDGET)?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    };
    Ok(Budget {
        state_ops: one("state_ops")?,
        searches: one("searches")?,
        egress: one("egress")?,
    })
}
