//! The store-level routes: create, delete and list the stores.

use oss_meta::MetaError;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::json;

use super::{DEFAULT, LtrService, Object, failed, index_name, missing};
use crate::route_util::refuse;

impl LtrService {
    /// `PUT /_ltr[/{store}]`.
    pub(crate) fn create_store(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let store = Self::store_of(req)?;
        let mut made = false;
        for object in [Object::FeatureSet, Object::Model] {
            made |= self.set(&store, object)?.create().map_err(|e| failed(&e))?;
        }
        if store != DEFAULT {
            let mut marker = self.marker(req, 0)?;
            match self.registry()?.put(&store, b"{}", true, &mut marker) {
                Ok(()) => made = true,
                Err(MetaError::AlreadyExists) => {}
                Err(e) => return Err(failed(&e)),
            }
        }
        if made {
            resp.send(200, JSON, b"{\"acknowledged\":true}")
        } else {
            Err(refuse(
                400,
                "resource_already_exists_exception",
                // ALLOC: error path; one message.
                format!("index [{}] already exists", index_name(&store)),
            ))
        }
    }

    /// `DELETE /_ltr[/{store}]`: every object, then the markers.
    pub(crate) fn delete_store(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let store = Self::store_of(req)?;
        let mut marker = self.marker(req, 0)?;
        for object in [Object::FeatureSet, Object::Model] {
            let set = self.set(&store, object)?;
            let names = match set.names(&mut marker) {
                Ok(n) => n,
                Err(MetaError::NoSuchSet) => return Err(missing(&store)),
                Err(e) => return Err(failed(&e)),
            };
            for name in names {
                set.delete(&name, &mut marker).map_err(|e| failed(&e))?;
            }
            set.destroy().map_err(|e| failed(&e))?;
        }
        if store != DEFAULT {
            self.registry()?
                .delete(&store, &mut marker)
                .map_err(|e| failed(&e))?;
        }
        resp.send(200, JSON, b"{\"acknowledged\":true}")
    }

    /// `GET /_ltr`: the stores, from the registry; the default store when
    /// its marker exists.
    pub(crate) fn list_stores(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let mut marker = self.marker(req, 0)?;
        let names = self
            .registry()?
            .names(&mut marker)
            .map_err(|e| failed(&e))?;
        let has_default = match self.set(DEFAULT, Object::FeatureSet)?.names(&mut marker) {
            Ok(_) => true,
            Err(MetaError::NoSuchSet) => false,
            Err(e) => return Err(failed(&e)),
        };
        let mut stores = serde_json::Map::new();
        for name in names {
            stores.insert(index_name(&name), json!({}));
        }
        let body = json!({"stores": stores, "default": has_default});
        resp.send(200, JSON, body.to_string().as_bytes())
    }
}
