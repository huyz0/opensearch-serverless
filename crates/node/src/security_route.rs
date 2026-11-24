//! `/_plugins/_security/api/{roles,rolesmapping,internalusers,tenants,actiongroups}/{name}`:
//! store, read and remove the security configuration, over five document
//! sets ([ADR 0043](../../../docs/adr/0043-security-configuration-as-document-sets.md)).
//!
//! Answers are the stock plugin's, recorded in `fixtures/compat/security/`;
//! what to validate and render is `oss-coordinator`'s `security`.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_coordinator::security::{
    ALL_ACCESS, Answer, Kind, authinfo, deleted, missing, needs_credentials, password_ok, render,
    saved, validate, weak_password, with_hash,
};
use oss_doc::mapping::json::{Json, parse};
use oss_format::{DocKind, MAX_DOC_BYTES};
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{read_body, refuse};
use crate::security_state::SecurityState;

/// The bcrypt cost the plugin's own hashes use.
pub const DEFAULT_COST: u32 = 12;

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
    fn admin_only(&self, req: &Request<'_>) -> Result<Option<Answer>, RestError> {
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
        let name = req.principal().unwrap_or_default();
        let roles = config.roles_of(name);
        if roles.iter().any(|r| r == ALL_ACCESS) {
            return Ok(None);
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
        Ok(Some(Answer {
            status: 403,
            body: format!(r#"{{"status":"FORBIDDEN","message":{text}}}"#),
        }))
    }

    fn changed(&self) {
        if let Some(state) = &self.state {
            state.invalidate();
        }
    }

    fn set(&self, kind: Kind) -> &DocSet {
        &self.sets[Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0)]
    }

    /// The stored document `name` of `kind`, if there is one.
    fn read(
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
        let mut stored = match validate(kind, &body) {
            Ok(doc) => doc,
            Err(answer) => return Ok(answer),
        };
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut doc = vec![0u8; self.buffer_bytes];
        let failed = |e: MetaError| failure(&e);
        if kind == Kind::RolesMapping
            && self
                .read(Kind::Roles, name, &mut marker, &mut doc)
                .map_err(failed)?
                .is_none()
        {
            return Ok(missing(Kind::Roles, name));
        }
        let existing = self
            .read(kind, name, &mut marker, &mut doc)
            .map_err(failed)?;
        if kind == Kind::InternalUsers {
            match self.credentials(&mut stored, existing.as_ref()) {
                Ok(()) => {}
                Err(answer) => return Ok(answer),
            }
        }
        self.set(kind)
            .put(name, stored.to_json().as_bytes(), false, &mut marker)
            .map_err(failed)?;
        self.changed();
        Ok(saved(name, existing.is_none()))
    }

    /// Leaves `stored` holding a hash: the password hashed, the given hash,
    /// or on an update the hash it already had.
    fn credentials(&self, stored: &mut Json, existing: Option<&Json>) -> Result<(), Answer> {
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
        let answer = match req.method() {
            Method::Put => self.put(kind, &name, req)?,
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
fn failure(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(400, "too_many_documents", e.to_string()),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => {
            refuse(400, "illegal_argument_exception", e.to_string())
        }
        // ALLOC: error path; one message.
        other => refuse(500, "security_config_failure", other.to_string()),
    }
}

/// The management routes: `GET`, `PUT` and `DELETE` of one document of each
/// kind.
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
        let service = Arc::clone(service);
        router = router.route(
            Method::Get,
            path,
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                if let Some(refused) = service.admin_only(req)? {
                    return resp.send(refused.status, JSON, refused.body.as_bytes());
                }
                Err(refuse(
                    501,
                    "not_implemented",
                    format!("listing {} is not served by this node", kind.segment()),
                ))
            },
        )?;
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
        for method in [Method::Get, Method::Put, Method::Delete] {
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
            let body = authinfo(&config, req.principal().unwrap_or_default());
            resp.send(200, JSON, body.as_bytes())
        },
    )
}
