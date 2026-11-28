//! `/_plugins/_security/api/{roles,rolesmapping,internalusers,tenants,actiongroups}/{name}`:
//! store, read and remove the security configuration, over five document
//! sets ([ADR 0043](../../../docs/adr/0043-security-configuration-as-document-sets.md)).
//!
//! Answers are the stock plugin's, recorded in `fixtures/compat/security/`;
//! what to validate and render is `oss-coordinator`'s `security`.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::security::{
    ALL_ACCESS, Answer, Config, Kind, Who, authinfo, builtin, deleted, missing, needs_credentials,
    password_ok, protection, read_builtin, render, saved, validate, weak_password, with_hash,
};
use oss_doc::mapping::json::{Json, parse};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

mod account;
mod admin;
mod patch;

pub use account::account_routes;
pub use admin::config_routes;

use crate::route_util::{read_body, refuse};
use crate::security_state::SecurityState;

/// The bcrypt cost the plugin's own hashes use.
pub const DEFAULT_COST: u32 = 12;

/// The most documents one bulk patch changes: each may hash a password.
pub const MAX_BULK_DOCS: usize = 100;

fn doc_kind(kind: Kind) -> DocKind {
    match kind {
        Kind::Roles => DocKind::SecurityRoles,
        Kind::RolesMapping => DocKind::SecurityRolesMapping,
        Kind::InternalUsers => DocKind::SecurityInternalUsers,
        Kind::Tenants => DocKind::SecurityTenants,
        Kind::ActionGroups => DocKind::SecurityActionGroups,
    }
}

/// Serves the management routes over the five sets.
pub struct SecurityService {
    sets: [DocSet; 5],
    buffer_bytes: usize,
    cost: u32,
    state: Option<Arc<SecurityState>>,
    jwt: Option<oss_coordinator::security::Rules>,
}

impl SecurityService {
    /// A service over the sets under `base` in `store`; each request holds
    /// two buffers of `buffer_bytes`, charged to the request-body pool, and
    /// passwords are hashed at bcrypt cost `cost`.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
        cost: u32,
    ) -> Result<Self, MetaError> {
        let set = |kind: Kind| DocSet::new(Arc::clone(store), base, doc_kind(kind));
        Ok(Self {
            sets: [
                set(Kind::Roles)?,
                set(Kind::RolesMapping)?,
                set(Kind::InternalUsers)?,
                set(Kind::Tenants)?,
                set(Kind::ActionGroups)?,
            ],
            buffer_bytes,
            cost,
            state: None,
            jwt: None,
        })
    }

    /// Serves the routes only to administrators, as `state` knows them, and
    /// tells it of every change so this node sees it at once.
    #[must_use]
    pub fn with_state(mut self, state: Arc<SecurityState>) -> Self {
        self.state = Some(state);
        self
    }

    /// The refusal for a caller who is not an administrator, if it is not.
    pub(super) fn admin_only(&self, req: &Request<'_>) -> Result<Option<Answer>, RestError> {
        let Some(state) = &self.state else {
            return Ok(None);
        };
        let config = state.config().map_err(|_| {
            refuse(
                503,
                "security_exception",
                "the security configuration is unavailable",
            )
        })?;
        Ok(admin_refusal(&config, who_of(req)))
    }

    /// Lists the JSON Web Token domain with `rules`, if any, in `securityconfig`.
    #[must_use]
    pub fn with_jwt_rules(mut self, rules: Option<oss_coordinator::security::Rules>) -> Self {
        self.jwt = rules;
        self
    }

    pub(super) fn jwt_rules(&self) -> Option<&oss_coordinator::security::Rules> {
        self.jwt.as_ref()
    }

    pub(super) fn changed(&self) {
        if let Some(state) = &self.state {
            state.invalidate();
        }
    }

    pub(super) fn set(&self, kind: Kind) -> &DocSet {
        &self.sets[Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0)]
    }

    /// The stored document `name` of `kind`, if there is one.
    pub(super) fn read(
        &self,
        kind: Kind,
        name: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Json>, MetaError> {
        let Some(len) = self.set(kind).get(name, marker, doc)? else {
            return Ok(None);
        };
        parse(&doc[..len])
            .map(Some)
            .map_err(|_| MetaError::InvalidArgument("a stored security document is not JSON"))
    }

    fn put(&self, kind: Kind, name: &str, req: &mut Request<'_>) -> Result<Answer, RestError> {
        let body = read_body(
            req,
            MAX_DOC_BYTES,
            "a security document is larger than 1 MiB",
        )?;
        let stored = match validate(kind, &body) {
            Ok(doc) => doc,
            Err(answer) => return Ok(answer),
        };
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut doc = vec![0u8; self.buffer_bytes];
        if let Some(refused) = self.role_missing(kind, name, &mut marker, &mut doc)? {
            return Ok(refused);
        }
        let existing = self
            .read(kind, name, &mut marker, &mut doc)
            .map_err(|e| failure(&e))?;
        self.commit(kind, name, stored, existing.as_ref(), &mut marker)
    }

    /// The refusal of a mapping whose role is not there.
    pub(super) fn role_missing(
        &self,
        kind: Kind,
        name: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Option<Answer>, RestError> {
        if kind != Kind::RolesMapping || builtin(Kind::Roles, name).is_some() {
            return Ok(None);
        }
        let there = self
            .read(Kind::Roles, name, marker, doc)
            .map_err(|e| failure(&e))?;
        Ok(there.is_none().then(|| missing(Kind::Roles, name)))
    }

    /// Stores the validated `stored` as `name`, hashing a user's password.
    fn commit(
        &self,
        kind: Kind,
        name: &str,
        mut stored: Json,
        existing: Option<&Json>,
        marker: &mut [u8],
    ) -> Result<Answer, RestError> {
        if kind == Kind::InternalUsers
            && let Err(answer) = self.credentials(&mut stored, existing)
        {
            return Ok(answer);
        }
        self.set(kind)
            .put(name, stored.to_json().as_bytes(), false, marker)
            .map_err(|e| failure(&e))?;
        self.changed();
        Ok(saved(name, existing.is_none()))
    }

    /// Leaves `stored` holding a hash: the password hashed, the given hash,
    /// or on an update the hash it already had.
    pub(super) fn credentials(
        &self,
        stored: &mut Json,
        existing: Option<&Json>,
    ) -> Result<(), Answer> {
        let text = |doc: &Json, key: &str| match doc.get(key) {
            Some(Json::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let hash = match (text(stored, "password"), text(stored, "hash")) {
            (Some(password), _) => {
                if !password_ok(&password) {
                    return Err(weak_password());
                }
                let hashed = bcrypt::hash(&password, self.cost).map_err(|_| refused_hash())?;
                Some(hashed)
            }
            (None, Some(hash)) => Some(hash),
            (None, None) => match existing.and_then(|e| text(e, "hash")) {
                Some(hash) => Some(hash),
                None => return Err(needs_credentials()),
            },
        };
        *stored = with_hash(std::mem::replace(stored, Json::Null), hash);
        Ok(())
    }

    fn handle(
        &self,
        kind: Kind,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let name = req
            .path_param("name")
            .map(|n| n.into_owned())
            .ok_or_else(|| RestError::illegal_argument("a name is required"))?;
        if let Some(refused) = self.admin_only(req)? {
            return resp.send(refused.status, JSON, refused.body.as_bytes());
        }
        if req.method() != Method::Get
            && let Some(refused) = protection(kind, &name)
        {
            return resp.send(refused.status, JSON, refused.body.as_bytes());
        }
        let answer = match req.method() {
            Method::Put => self.put(kind, &name, req)?,
            Method::Patch => self.patch(kind, &name, req)?,
            Method::Delete => {
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let there = self
                    .set(kind)
                    .delete(&name, &mut marker)
                    .map_err(|e| failure(&e))?;
                self.changed();
                if there {
                    deleted(&name)
                } else {
                    missing(kind, &name)
                }
            }
            _ => {
                if let Some(shipped) = read_builtin(kind, &name) {
                    return resp.send(200, JSON, shipped.as_bytes());
                }
                req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
                // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let mut doc = vec![0u8; self.buffer_bytes];
                match self
                    .read(kind, &name, &mut marker, &mut doc)
                    .map_err(|e| failure(&e))?
                {
                    Some(stored) => Answer {
                        status: 200,
                        body: render(kind, &name, &stored),
                    },
                    None => missing(kind, &name),
                }
            }
        };
        resp.send(answer.status, JSON, answer.body.as_bytes())
    }
}

fn refused_hash() -> Answer {
    Answer {
        status: 500,
        body: r#"{"status":"error","reason":"The password could not be hashed"}"#.to_owned(),
    }
}

/// What a failed store call answers.
pub(super) fn failure(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(400, "too_many_documents", e.to_string()),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => {
            refuse(400, "illegal_argument_exception", e.to_string())
        }
        // ALLOC: error path; one message.
        other => refuse(500, "security_config_failure", other.to_string()),
    }
}

/// The management routes: `GET`, `PUT`, `PATCH` and `DELETE` of one document
/// of each kind, and `GET` and `PATCH` of the kind.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<SecurityService>) -> Result<Router, RouteError> {
    let mut router = router;
    const LISTS: [(Kind, &str); 5] = [
        (Kind::Roles, "/_plugins/_security/api/roles"),
        (Kind::RolesMapping, "/_plugins/_security/api/rolesmapping"),
        (Kind::InternalUsers, "/_plugins/_security/api/internalusers"),
        (Kind::Tenants, "/_plugins/_security/api/tenants"),
        (Kind::ActionGroups, "/_plugins/_security/api/actiongroups"),
    ];
    for (kind, path) in LISTS {
        for method in [Method::Get, Method::Patch] {
            let service = Arc::clone(service);
            router = router.route(
                method,
                path,
                &[],
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    if let Some(refused) = service.admin_only(req)? {
                        return resp.send(refused.status, JSON, refused.body.as_bytes());
                    }
                    let answer = if req.method() == Method::Patch {
                        service.bulk_patch(kind, req)?
                    } else {
                        service.read_all(kind, req)?
                    };
                    resp.send(answer.status, JSON, answer.body.as_bytes())
                },
            )?;
        }
    }
    const PATHS: [(Kind, &str); 5] = [
        (Kind::Roles, "/_plugins/_security/api/roles/{name}"),
        (
            Kind::RolesMapping,
            "/_plugins/_security/api/rolesmapping/{name}",
        ),
        (
            Kind::InternalUsers,
            "/_plugins/_security/api/internalusers/{name}",
        ),
        (Kind::Tenants, "/_plugins/_security/api/tenants/{name}"),
        (
            Kind::ActionGroups,
            "/_plugins/_security/api/actiongroups/{name}",
        ),
    ];
    for (kind, path) in PATHS {
        for method in [Method::Get, Method::Put, Method::Patch, Method::Delete] {
            let service = Arc::clone(service);
            router = router.route(
                method,
                path,
                &[],
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle(kind, req, resp)
                },
            )?;
        }
    }
    Ok(router)
}

/// Every security route: the document API, `authinfo`, the configuration
/// routes and the account routes.
///
/// # Errors
///
/// A route registered twice.
pub fn all(
    router: Router,
    service: &Arc<SecurityService>,
    state: &Arc<SecurityState>,
) -> Result<Router, RouteError> {
    let router = routes(router, service)?;
    let router = authinfo_route(router, state)?;
    let router = config_routes(router, service, state)?;
    account_routes(router, service, state)
}

/// `GET /_plugins/_security/authinfo`: who the caller is, its roles and
/// tenants, as the plugin answers.
///
/// # Errors
///
/// A route registered twice.
pub fn authinfo_route(router: Router, state: &Arc<SecurityState>) -> Result<Router, RouteError> {
    let state = Arc::clone(state);
    router.route(
        Method::Get,
        "/_plugins/_security/authinfo",
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| {
            let config = state.config().map_err(|_| {
                refuse(
                    503,
                    "security_exception",
                    "the security configuration is unavailable",
                )
            })?;
            let body = authinfo(&config, who_of(req));
            resp.send(200, JSON, body.as_bytes())
        },
    )
}

/// Who `req` acts as; no one when no guard ran.
pub(crate) fn who_of<'r>(req: &'r Request<'_>) -> Who<'r> {
    req.identity()
        .map_or(Who::stored(""), crate::security_who::who)
}

/// The plugin's refusal of `name` for a management route, unless `name`
/// holds `all_access`.
pub fn admin_refusal(config: &Config, who: Who<'_>) -> Option<Answer> {
    let name = who.name;
    let roles = config.roles_of(who);
    if roles.iter().any(|r| r == ALL_ACCESS) {
        return None;
    }
    // ALLOC: one message on a refused request.
    let mut text = String::new();
    oss_doc::mapping::json::write_str(
        &mut text,
        &format!(
            "No permission to access REST API: User {name} with Security roles [{}] does not have any role privileged for admin access. No client TLS certificate found in request",
            roles.join(", ")
        ),
    );
    Some(Answer {
        status: 403,
        body: format!(r#"{{"status":"FORBIDDEN","message":{text}}}"#),
    })
}
