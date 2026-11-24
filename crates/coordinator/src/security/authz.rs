//! What a user may do: the roles' cluster and index permissions, the action
//! groups they name, and the plugin's wildcard rule
//! ([ADR 0051](../../../../docs/adr/0051-authorization-at-the-route-by-action-table.md)).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use oss_doc::mapping::json::{Json, parse};

/// The stock plugin's built-in action groups and roles, as shipped.
const STATIC: &str = include_str!("static_config.json");

/// A permission on indices: the patterns it covers and the actions it grants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexPermission {
    /// Index patterns (`*` and `?`, and `${user.name}`).
    pub patterns: Vec<String>,
    /// Action names, wildcard patterns or action group names.
    pub actions: Vec<String>,
    /// Whether the permission carries document-level security, field-level
    /// security or masked fields, none of which this node enforces: such a
    /// permission grants nothing, so a role is never looser than the plugin's.
    pub restricted: bool,
}

/// A role's permissions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Role {
    /// Cluster-level actions or groups.
    pub cluster: Vec<String>,
    /// Index-level permissions.
    pub indices: Vec<IndexPermission>,
}

/// The roles and action groups of one snapshot, built-ins included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authz {
    roles: BTreeMap<String, Role>,
    groups: BTreeMap<String, Vec<String>>,
}

fn strings(doc: &Json, key: &str) -> Vec<String> {
    match doc.get(key) {
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(|i| match i {
                Json::Str(s) => Some(s.clone()),
                _ => None,
            })
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            .collect(),
        // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
        _ => Vec::new(),
    }
}

fn role_of(doc: &Json) -> Role {
    let indices = match doc.get("index_permissions") {
        Some(Json::Arr(items)) => items
            .iter()
            .map(|p| IndexPermission {
                patterns: strings(p, "index_patterns"),
                actions: strings(p, "allowed_actions"),
                restricted: ["dls", "fls", "masked_fields"]
                    .iter()
                    .any(|k| match p.get(k) {
                        Some(Json::Str(s)) => !s.is_empty(),
                        Some(Json::Arr(a)) => !a.is_empty(),
                        Some(Json::Null) | None => false,
                        Some(_) => true,
                    }),
            })
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            .collect(),
        // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
        _ => Vec::new(),
    };
    Role {
        cluster: strings(doc, "cluster_permissions"),
        indices,
    }
}

/// The shipped roles and action groups.
type Shipped = (BTreeMap<String, Role>, BTreeMap<String, Vec<String>>);

fn built_in() -> &'static Shipped {
    static BUILT_IN: OnceLock<Shipped> = OnceLock::new();
    BUILT_IN.get_or_init(|| {
        let doc = parse(STATIC.as_bytes()).unwrap_or(Json::Null);
        // ALLOC: built once per process, from the static configuration.
        let mut roles = BTreeMap::new();
        if let Some(Json::Obj(members)) = doc.get("roles") {
            for (name, role) in members {
                roles.insert(name.clone(), role_of(role));
            }
        }
        // ALLOC: built once per process, from the static configuration.
        let mut groups = BTreeMap::new();
        if let Some(Json::Obj(members)) = doc.get("action_groups") {
            for (name, actions) in members {
                let list = match actions {
                    Json::Arr(items) => items
                        .iter()
                        .filter_map(|i| match i {
                            Json::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                        .collect(),
                    // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                    _ => Vec::new(),
                };
                groups.insert(name.clone(), list);
            }
        }
        (roles, groups)
    })
}

/// One element of a request's index expression: a character, a `?` or a `*`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok {
    Lit(char),
    One,
    Many,
}

/// Whether `pattern` (`*` any run, `?` one character) matches every string
/// the request expression `text` can stand for, a `*` or `?` in `text`
/// being a wildcard of the request, not a character.
// ARITH: indices into the two buffers, each stepped by one and bounded by their lengths.
#[allow(clippy::arithmetic_side_effects)]
fn covers(pattern: &str, text: &str) -> bool {
    let t: Vec<Tok> = text
        .chars()
        .map(|c| match c {
            '*' => Tok::Many,
            '?' => Tok::One,
            c => Tok::Lit(c),
        })
        // ALLOC: one request's index name, at most the path's length.
        .collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some('?') if t[ti] != Tok::Many => {
                pi += 1;
                ti += 1;
            }
            Some(c) if t[ti] == Tok::Lit(*c) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == p.len()
}

/// Whether the action name `action` matches `pattern` (`*` and `?`).
// ARITH: indices into the two buffers, each stepped by one and bounded by their lengths.
#[allow(clippy::arithmetic_side_effects)]
fn action_matches(pattern: &str, action: &str) -> bool {
    // ALLOC: two buffers the length of the pattern and the action name, both short.
    let p: Vec<char> = pattern.chars().collect();
    // ALLOC: two buffers the length of the pattern and the action name, both short.
    let t: Vec<char> = action.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(c) if *c == '?' || *c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == p.len()
}

impl Default for Authz {
    fn default() -> Self {
        Self::new()
    }
}

impl Authz {
    /// The built-ins alone.
    pub fn new() -> Self {
        let (roles, groups) = built_in();
        Self {
            roles: roles.clone(),
            groups: groups.clone(),
        }
    }

    /// Adds a stored role; a built-in of that name is not replaced.
    pub fn add_role(&mut self, name: &str, doc: &Json) {
        if !built_in().0.contains_key(name) {
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            self.roles.insert(name.to_owned(), role_of(doc));
        }
    }

    /// Adds a stored action group; a built-in of that name is not replaced.
    pub fn add_group(&mut self, name: &str, doc: &Json) {
        if !built_in().1.contains_key(name) {
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            self.groups
                .insert(name.to_owned(), strings(doc, "allowed_actions"));
        }
    }

    /// Whether any of `entries` (action names, wildcards or group names,
    /// groups to any depth) grants `action`.
    fn grants(&self, entries: &[String], action: &str) -> bool {
        let mut seen = BTreeSet::new();
        self.grants_in(entries, action, &mut seen)
    }

    fn grants_in<'a>(
        &'a self,
        entries: &'a [String],
        action: &str,
        seen: &mut BTreeSet<&'a str>,
    ) -> bool {
        entries.iter().any(|entry| match self.groups.get(entry) {
            Some(members) => seen.insert(entry) && self.grants_in(members, action, seen),
            None => action_matches(entry, action),
        })
    }

    /// Whether `roles` hold what `all_access` holds: the cluster permission
    /// `*` and an index permission over `*` granting `*`, written as such.
    pub fn all_access(&self, roles: &[String]) -> bool {
        let held = || roles.iter().filter_map(|r| self.roles.get(r));
        held().any(|r| r.cluster.iter().any(|c| c == "*"))
            && held().any(|r| {
                r.indices.iter().filter(|p| !p.restricted).any(|p| {
                    p.patterns.iter().any(|x| x == "*") && p.actions.iter().any(|a| a == "*")
                })
            })
    }

    /// Whether any of `roles` grants the cluster-level `action`.
    pub fn cluster_allows(&self, roles: &[String], action: &str) -> bool {
        roles
            .iter()
            .filter_map(|r| self.roles.get(r))
            .any(|r| self.grants(&r.cluster, action))
    }

    /// Whether any of `roles` grants `action` on every index in `indices`,
    /// each index (a name or an expression with `*`) covered by one
    /// permission's pattern that grants the action. No index is the request
    /// for all of them. `user` replaces `${user.name}` in a pattern, except
    /// that a name holding a wildcard voids the pattern.
    pub fn indices_allow(
        &self,
        roles: &[String],
        user: &str,
        action: &str,
        indices: &[&str],
    ) -> bool {
        let all = ["*"];
        let wanted: &[&str] = if indices.is_empty() { &all } else { indices };
        wanted.iter().all(|index| {
            roles
                .iter()
                .filter_map(|r| self.roles.get(r))
                .flat_map(|r| r.indices.iter())
                .filter(|p| !p.restricted && self.grants(&p.actions, action))
                .flat_map(|p| p.patterns.iter())
                .any(|pattern| {
                    if pattern.contains("${user.name}") {
                        !user.contains(['*', '?'])
                            && covers(&pattern.replace("${user.name}", user), index)
                    } else {
                        covers(pattern, index)
                    }
                })
        })
    }
}
