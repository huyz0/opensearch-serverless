//! `GET` and `PUT /_plugins/_security/api/account`: a user reads what it is
//! and changes its own password (docs/specs/security-complete/SPEC.md,
//! slice 4). Open to every authenticated user, and only ever about the
//! caller.

use std::sync::Arc;

use oss_coordinator::security::{
    Kind, account, account_has_no_password, account_reserved, password_change, wrong_current,
};
use oss_doc::mapping::json::Json;
use oss_format::MAX_DOC_BYTES;
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{SecurityService, failure};
use crate::route_util::{read_body, refuse};
use crate::security_state::SecurityState;

const PATH: &str = "/_plugins/_security/api/account";

/// Registers `GET` and `PUT` of the account route.
///
/// # Errors
///
/// A route registered twice.
pub fn account_routes(
    router: Router,
    service: &Arc<SecurityService>,
    state: &Arc<SecurityState>,
) -> Result<Router, RouteError> {
    let held = Arc::clone(state);
    let router = router.route(
        Method::Get,
        PATH,
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| {
            let config = held.config().map_err(|_| unavailable())?;
            let body = account(&config, super::who_of(req));
            resp.send(200, JSON, body.as_bytes())
        },
    )?;
    let (write, held) = (Arc::clone(service), Arc::clone(state));
    router.route(
        Method::Put,
        PATH,
        &[],
        move |req: &mut Request<'_>, resp: &mut Response<'_>| {
            let config = held.config().map_err(|_| unavailable())?;
            let name = req.principal().unwrap_or_default().to_owned();
            if req.identity().is_some_and(|i| i.token_roles().is_some()) {
                let refused = account_has_no_password();
                return resp.send(refused.status, JSON, refused.body.as_bytes());
            }
            let answer = write.change_password(&config, &name, req)?;
            resp.send(answer.status, JSON, answer.body.as_bytes())
        },
    )
}

fn unavailable() -> RestError {
    refuse(
        503,
        "security_exception",
        "the security configuration is unavailable",
    )
}

impl SecurityService {
    fn change_password(
        &self,
        config: &oss_coordinator::security::Config,
        name: &str,
        req: &mut Request<'_>,
    ) -> Result<oss_coordinator::security::Answer, RestError> {
        let body = read_body(req, MAX_DOC_BYTES, "a password change is larger than 1 MiB")?;
        let (current, new) = match password_change(&body) {
            Ok(pair) => pair,
            Err(answer) => return Ok(answer),
        };
        if config.is_admin(name) {
            return Ok(account_reserved(name));
        }
        let hash = config
            .user(name)
            .map(|u| u.hash.as_str())
            .unwrap_or_default();
        if !bcrypt::verify(&current, hash).unwrap_or(false) {
            return Ok(wrong_current());
        }
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut doc = vec![0u8; self.buffer_bytes];
        let Some(existing) = self
            .read(Kind::InternalUsers, name, &mut marker, &mut doc)
            .map_err(|e| failure(&e))?
        else {
            return Ok(wrong_current());
        };
        let Json::Obj(members) = &existing else {
            return Ok(wrong_current());
        };
        // ALLOC: one copy of the stored user, at most 1 MiB.
        let mut stored: Vec<(String, Json)> = members
            .iter()
            .filter(|(k, _)| k != "hash")
            .cloned()
            .collect();
        stored.push(("password".to_owned(), Json::Str(new)));
        self.commit(
            Kind::InternalUsers,
            name,
            Json::Obj(stored),
            Some(&existing),
            &mut marker,
        )
    }
}
