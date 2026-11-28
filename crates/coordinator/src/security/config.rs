//! The security configuration a node holds: the users, role mappings and
//! tenants read from the five document sets, and which roles a user has.

use std::collections::BTreeMap;

use oss_doc::mapping::json::Json;

use super::Kind;
use super::authz::Authz;

/// The built-in role a static administrator holds.
pub const ALL_ACCESS: &str = "all_access";

/// Who a request acts as: a name, and for a token-authenticated request the
/// backend roles its token carried. A token identity takes its roles only
/// from those, never from a stored user of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Who<'a> {
    /// The user's name.
    pub name: &'a str,
    /// The token's backend roles; `None` for a stored user.
    pub token: Option<&'a [String]>,
}

impl<'a> Who<'a> {
    /// A stored user.
    pub fn stored(name: &'a str) -> Self {
        Self { name, token: None }
    }

    /// A user a token vouched for, holding `backend_roles`.
    pub fn token(name: &'a str, backend_roles: &'a [String]) -> Self {
        Self {
            name,
            token: Some(backend_roles),
        }
    }
}

impl<'a> From<&'a str> for Who<'a> {
    fn from(name: &'a str) -> Self {
        Self::stored(name)
    }
}

/// An internal user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct User {
    /// The bcrypt hash of the password.
    pub hash: String,
    /// The user's backend roles.
    pub backend_roles: Vec<String>,
    /// Roles the user holds directly (`opendistro_security_roles`).
    pub roles: Vec<String>,
    /// Names of the user's attributes.
    pub attributes: Vec<String>,
}

/// Who a role is granted to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapping {
    /// User names (`*` is every user).
    pub users: Vec<String>,
    /// Backend roles, any one of which grants the role.
    pub backend_roles: Vec<String>,
    /// Backend roles, all of which are needed.
    pub and_backend_roles: Vec<String>,
}

/// Everything authentication reads, as of one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    users: BTreeMap<String, User>,
    mappings: BTreeMap<String, Mapping>,
    tenants: Vec<String>,
    admin: Option<String>,
    authz: Authz,
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

impl Config {
    /// An empty configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `name` the static administrator, holding [`ALL_ACCESS`] with
    /// the password that hashes to `hash`, in place of any stored user of
    /// that name.
    #[must_use]
    pub fn with_admin(mut self, name: &str, hash: &str) -> Self {
        self.users.insert(
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            name.to_owned(),
            User {
                // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                hash: hash.to_owned(),
                // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                backend_roles: vec!["admin".to_owned()],
                // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                roles: vec![ALL_ACCESS.to_owned()],
                // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                attributes: Vec::new(),
            },
        );
        // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
        self.admin = Some(name.to_owned());
        self
    }

    /// Adds one stored document; a kind authentication does not read is
    /// ignored, and so is a user the static administrator shadows.
    pub fn add(&mut self, kind: Kind, name: &str, doc: &Json) {
        match kind {
            Kind::InternalUsers if self.admin.as_deref() != Some(name) => {
                let attributes = match doc.get("attributes") {
                    // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                    Some(Json::Obj(members)) => members.iter().map(|(k, _)| k.clone()).collect(),
                    // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                    _ => Vec::new(),
                };
                self.users.insert(
                    // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                    name.to_owned(),
                    User {
                        hash: match doc.get("hash") {
                            Some(Json::Str(h)) => h.clone(),
                            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                            _ => String::new(),
                        },
                        backend_roles: strings(doc, "backend_roles"),
                        roles: strings(doc, "opendistro_security_roles"),
                        attributes,
                    },
                );
            }
            Kind::RolesMapping => {
                self.mappings.insert(
                    // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
                    name.to_owned(),
                    Mapping {
                        users: strings(doc, "users"),
                        backend_roles: strings(doc, "backend_roles"),
                        and_backend_roles: strings(doc, "and_backend_roles"),
                    },
                );
            }
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            Kind::Tenants => self.tenants.push(name.to_owned()),
            Kind::Roles => self.authz.add_role(name, doc),
            Kind::ActionGroups => self.authz.add_group(name, doc),
            Kind::InternalUsers => {}
        }
    }

    /// The user `name`, if there is one.
    pub fn user(&self, name: &str) -> Option<&User> {
        self.users.get(name)
    }

    /// Whether `who` is the static administrator. A token identity never is.
    pub fn is_admin<'a>(&self, who: impl Into<Who<'a>>) -> bool {
        let who = who.into();
        who.token.is_none() && self.admin.as_deref() == Some(who.name)
    }

    /// The backend roles of `who`: the token's for a token identity, the
    /// stored user's otherwise.
    pub fn backend_roles_of<'a>(&'a self, who: impl Into<Who<'a>>) -> &'a [String] {
        let who = who.into();
        match who.token {
            Some(roles) => roles,
            None => self
                .users
                .get(who.name)
                .map_or(&[][..], |u| u.backend_roles.as_slice()),
        }
    }

    /// The roles `who` holds, sorted: for a stored user its own, and those
    /// of every mapping that names it, one of its backend roles, or all of
    /// the mapping's `and_backend_roles`; for a token identity only the
    /// mappings of its backend roles (a mapping's `users` list never
    /// applies to a token).
    pub fn roles_of<'a>(&self, who: impl Into<Who<'a>>) -> Vec<String> {
        let who = who.into();
        let user = self.users.get(who.name);
        if who.token.is_none() && user.is_none() {
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            return Vec::new();
        }
        let backend = self.backend_roles_of(who);
        let has = |role: &String| backend.contains(role) || (role == "*" && !backend.is_empty());
        let mut roles = match (who.token, user) {
            (None, Some(u)) => u.roles.clone(),
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            _ => Vec::new(),
        };
        for (role, m) in &self.mappings {
            let by_name = who.token.is_none() && m.users.iter().any(|u| u == who.name || u == "*");
            let by_backend = m.backend_roles.iter().any(has);
            let by_all = !m.and_backend_roles.is_empty()
                && m.and_backend_roles.iter().all(|r| backend.contains(r));
            if by_name || by_backend || by_all {
                roles.push(role.clone());
            }
        }
        roles.sort();
        roles.dedup();
        roles
    }

    /// Whether `who` holds what `all_access` holds.
    pub fn all_access<'a>(&self, who: impl Into<Who<'a>>) -> bool {
        self.authz.all_access(&self.roles_of(who))
    }

    /// Whether `who` may take the cluster-level `action`.
    pub fn cluster_allows<'a>(&self, who: impl Into<Who<'a>>, action: &str) -> bool {
        self.authz.cluster_allows(&self.roles_of(who), action)
    }

    /// Whether `who` may take `action` on every one of `indices` (none
    /// meaning all of them).
    pub fn indices_allow<'a>(
        &self,
        who: impl Into<Who<'a>>,
        action: &str,
        indices: &[&str],
    ) -> bool {
        let who = who.into();
        self.authz
            .indices_allow(&self.roles_of(who), who.name, action, indices)
    }

    /// The tenants `name` can use: its own private one, and for the
    /// administrator the global, admin and every stored tenant too.
    pub fn tenants_of<'a>(&self, who: impl Into<Who<'a>>) -> Vec<String> {
        let who = who.into();
        // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
        let mut tenants = vec![who.name.to_owned()];
        if self.is_admin(who) {
            // ALLOC: a refresh copies each stored document once, at most 1000 a kind.
            tenants.extend(["global_tenant".to_owned(), "admin_tenant".to_owned()]);
            tenants.extend(self.tenants.iter().cloned());
        }
        tenants.sort();
        tenants.dedup();
        tenants
    }
}
