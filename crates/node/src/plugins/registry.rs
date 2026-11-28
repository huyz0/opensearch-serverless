//! The registry: each plugin the policy names, loaded or refused with a
//! reason, and the rules that decide which (docs/specs/plugin-system AC-3,
//! AC-4, AC-5). A refusal keeps the node serving without that plugin.

use std::collections::HashMap;
use std::sync::Arc;

use oss_ext::breaker::BreakerConfig;
use oss_ext::manifest::{Manifest, ManifestError};
use oss_ext::native::{Guard, NativeInstance};
use oss_ext::{Capability, Tier};

use super::policy::{Entry, Policy, PolicyTier};

/// Whether a plugin serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Loaded; its routes are mounted.
    Ready,
    /// Not loaded, and why.
    Refused(String),
}

/// A route already taken: a built-in's, or an earlier plugin's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// The method, upper case; none for a route that takes any.
    pub method: Option<String>,
    /// The pattern.
    pub pattern: String,
    /// Who holds it, for the refusal's words.
    pub holder: String,
}

/// One plugin the policy named.
pub struct Plugin {
    entry: Entry,
    state: State,
    manifest: Option<Manifest>,
    instance: Option<Arc<dyn NativeInstance>>,
    guard: Guard,
}

impl Plugin {
    /// The plugin's id.
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    /// Its policy entry.
    pub fn entry(&self) -> &Entry {
        &self.entry
    }

    /// Whether it serves.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Its manifest, when it parsed.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }

    /// The code to call, when it loaded.
    pub fn instance(&self) -> Option<&Arc<dyn NativeInstance>> {
        self.instance.as_ref()
    }

    /// The bulkhead, breaker and panic fence around its calls.
    pub fn guard(&self) -> &Guard {
        &self.guard
    }
}

/// Every plugin the policy named.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<Arc<Plugin>>,
}

fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

fn same_shape(a: &str, b: &str) -> bool {
    let (a, b) = (segments(a), segments(b));
    let param = |s: &str| s.starts_with('{');
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(x, y)| (param(x) && param(y)) || x == y)
}

/// Whether `path` is under `prefix` on whole segments.
fn under(path: &str, prefix: &str) -> bool {
    let (p, q) = (segments(path), segments(prefix));
    p.len() >= q.len() && p.iter().zip(&q).all(|(x, y)| x == y)
}

fn refuse<T>(why: impl Into<String>) -> Result<T, String> {
    Err(why.into())
}

/// Judges one entry: the manifest and routes it may have, or why not.
fn judge(
    entry: &Entry,
    linked: &HashMap<String, Arc<dyn NativeInstance>>,
    taken: &[Taken],
) -> Result<(Manifest, Arc<dyn NativeInstance>), String> {
    if entry.tier != PolicyTier::Native {
        return refuse("this tier is not available in this build");
    }
    let Some(instance) = linked.get(&entry.name) else {
        return refuse(format!("no plugin is registered as `{}`", entry.name));
    };
    let manifest = match Manifest::parse(&instance.manifest()) {
        Ok(m) => m,
        Err(ManifestError::Api(e)) => return refuse(format!("plugin api: {e:?}")),
        Err(ManifestError::Invalid(e)) => {
            return refuse(format!("manifest: {}: {}", e.key(), e.reason()));
        }
    };
    if manifest.id() != entry.id {
        return refuse("the manifest's id is not the policy's");
    }
    if let Err(e) = manifest.for_tier(Tier::Native) {
        return refuse(format!("manifest: {}: {}", e.key(), e.reason()));
    }
    for c in manifest.capabilities() {
        if !entry.grants.contains(c) {
            return refuse(format!(
                "asks for {} which the policy does not grant",
                name_of(c)
            ));
        }
    }
    let home = format!("/_plugins/{}", entry.id);
    for r in manifest.routes() {
        let allowed = under(r.path(), &home) || entry.claims.iter().any(|c| under(r.path(), c));
        if !allowed {
            return refuse(format!(
                "route {} is outside {home}/ and not claimed",
                r.path()
            ));
        }
        if let Some(t) = taken.iter().find(|t| {
            t.method.as_deref().is_none_or(|m| m == r.method()) && same_shape(&t.pattern, r.path())
        }) {
            return refuse(format!(
                "route {} {} collides with {} {}",
                r.method(),
                r.path(),
                t.holder,
                t.pattern
            ));
        }
    }
    for (key, _) in &entry.actions {
        let known = manifest
            .routes()
            .iter()
            .any(|r| *key == format!("{} {}", r.method(), r.path()));
        if !known {
            return refuse(format!(
                "action row `{key}` names no route the plugin declares"
            ));
        }
    }
    Ok((manifest, Arc::clone(instance)))
}

fn name_of(c: &Capability) -> String {
    match c {
        Capability::StateRead => "state.read".to_owned(),
        Capability::StateWrite => "state.write".to_owned(),
        Capability::Search => "search".to_owned(),
        Capability::Egress(h) => format!("egress:{h}"),
        Capability::Identity => "identity".to_owned(),
        Capability::Log => "log".to_owned(),
        Capability::Metrics => "metrics".to_owned(),
    }
}

fn seed_of(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Registry {
    /// Loads every entry of `policy` against the plugins linked into this
    /// build and the routes `builtin` already holds. An entry that fails is
    /// kept, refused, and holds no route.
    pub fn load(
        policy: &Policy,
        linked: &HashMap<String, Arc<dyn NativeInstance>>,
        builtin: &[Taken],
    ) -> Self {
        // ALLOC: at most the policy's MAX_PLUGINS entries, once at start.
        let mut taken: Vec<Taken> = builtin.to_vec();
        let mut plugins = Vec::new();
        for entry in policy.entries() {
            let guard = Guard::new(BreakerConfig::default(), seed_of(&entry.id));
            let (state, manifest, instance) = match judge(entry, linked, &taken) {
                Ok((m, i)) => {
                    for r in m.routes() {
                        taken.push(Taken {
                            method: Some(r.method().to_owned()),
                            pattern: r.path().to_owned(),
                            holder: format!("plugin {}", entry.id),
                        });
                    }
                    (State::Ready, Some(m), Some(i))
                }
                Err(why) => (State::Refused(why), None, None),
            };
            plugins.push(Arc::new(Plugin {
                entry: entry.clone(),
                state,
                manifest,
                instance,
                guard,
            }));
        }
        Self { plugins }
    }

    /// Every plugin, in policy order.
    pub fn plugins(&self) -> &[Arc<Plugin>] {
        &self.plugins
    }
}
