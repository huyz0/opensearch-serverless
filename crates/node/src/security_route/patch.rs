//! `GET` of a whole kind and `PATCH`, of one document or of a kind
//! (docs/specs/security-complete/SPEC.md, slice 1).

use oss_coordinator::security::{
    Answer, Kind, MAX_LIST_BYTES, apply_ops, bulk_updated, missing, patch_document, plan_bulk,
    render_list, validate,
};
use oss_doc::mapping::json::{Json, parse};
use oss_format::MAX_DOC_BYTES;
use oss_rest::{Request, RestError};

use super::{MAX_BULK_DOCS, SecurityService, failure};
use crate::route_util::{read_body, refuse};

impl SecurityService {
    /// `PATCH` of one stored document with an RFC 6902 array.
    pub(super) fn patch(
        &self,
        kind: Kind,
        name: &str,
        req: &mut Request<'_>,
    ) -> Result<Answer, RestError> {
        let body = read_body(req, MAX_DOC_BYTES, "a patch is larger than 1 MiB")?;
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut doc = vec![0u8; self.buffer_bytes];
        let Some(existing) = self
            .read(kind, name, &mut marker, &mut doc)
            .map_err(|e| failure(&e))?
        else {
            return Ok(missing(kind, name));
        };
        let stored = match patch_document(kind, existing.clone(), &body) {
            Ok(stored) => stored,
            Err(answer) => return Ok(answer),
        };
        self.commit(kind, name, stored, Some(&existing), &mut marker)
    }

    /// `PATCH` of a kind: operations whose paths start with document names.
    /// Every document is checked before the first is written.
    pub(super) fn bulk_patch(
        &self,
        kind: Kind,
        req: &mut Request<'_>,
    ) -> Result<Answer, RestError> {
        let body = read_body(req, MAX_DOC_BYTES, "a patch is larger than 1 MiB")?;
        let plan = match plan_bulk(&body) {
            Ok(plan) => plan,
            Err(answer) => return Ok(answer),
        };
        if plan.len() > MAX_BULK_DOCS {
            return Ok(Answer {
                status: 400,
                // ALLOC: error path; one message.
                body: format!(
                    r#"{{"status":"BAD_REQUEST","message":"A bulk patch changes at most {MAX_BULK_DOCS} documents."}}"#
                ),
            });
        }
        req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
        // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut doc = vec![0u8; self.buffer_bytes];
        // ALLOC: one entry a changed document, at most MAX_BULK_DOCS.
        let mut writes: Vec<(String, Option<Json>, Option<Json>)> = Vec::new();
        for (name, ops) in plan {
            let existing = self
                .read(kind, &name, &mut marker, &mut doc)
                .map_err(|e| failure(&e))?;
            let result = match apply_ops(existing.clone(), &ops) {
                Ok(result) => result,
                Err(answer) => return Ok(answer),
            };
            let Some(patched) = result else {
                writes.push((name, existing, None));
                continue;
            };
            let mut stored = match validate(kind, patched.to_json().as_bytes()) {
                Ok(stored) => stored,
                Err(answer) => return Ok(answer),
            };
            if let Some(refused) = self.role_missing(kind, &name, &mut marker, &mut doc)? {
                return Ok(refused);
            }
            if kind == Kind::InternalUsers
                && let Err(answer) = self.credentials(&mut stored, existing.as_ref())
            {
                return Ok(answer);
            }
            writes.push((name, existing, Some(stored)));
        }
        for (name, existing, stored) in writes {
            match stored {
                Some(stored) => {
                    self.set(kind)
                        .put(&name, stored.to_json().as_bytes(), false, &mut marker)
                        .map_err(|e| failure(&e))?;
                }
                None if existing.is_some() => {
                    self.set(kind)
                        .delete(&name, &mut marker)
                        .map_err(|e| failure(&e))?;
                }
                None => {}
            }
        }
        self.changed();
        Ok(bulk_updated())
    }

    /// Every document of a kind, as a read renders it, in name order.
    pub(super) fn read_all(&self, kind: Kind, req: &mut Request<'_>) -> Result<Answer, RestError> {
        req.body().reserve(
            self.buffer_bytes
                .saturating_mul(2)
                .saturating_add(MAX_LIST_BYTES),
        )?;
        // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
        let mut marker = vec![0u8; self.buffer_bytes];
        let mut doc = vec![0u8; self.buffer_bytes];
        // ALLOC: the answer, at most MAX_LIST_BYTES (reserved above) before it is refused.
        let mut out = String::from("{");
        let mut too_large = false;
        self.set(kind)
            .for_each(&mut marker, &mut doc, &mut |name, bytes| {
                if too_large {
                    return;
                }
                let Ok(json) = parse(bytes) else { return };
                let one = render_list(kind, &[(name.to_owned(), json)]);
                let inner = one.get(1..one.len().saturating_sub(1)).unwrap_or_default();
                if out.len().saturating_add(inner.len()) >= MAX_LIST_BYTES {
                    too_large = true;
                    return;
                }
                if out.len() > 1 {
                    out.push(',');
                }
                out.push_str(inner);
            })
            .map_err(|e| failure(&e))?;
        if too_large {
            return Err(refuse(
                413,
                "security_list_too_large",
                "the list is larger than 4 MiB; read the documents by name",
            ));
        }
        out.push('}');
        Ok(Answer {
            status: 200,
            body: out,
        })
    }
}
