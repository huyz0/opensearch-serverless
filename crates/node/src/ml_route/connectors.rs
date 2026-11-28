//! Connectors: the definitions a remote model calls through, one document each
//! of the `ml/connectors/` set. A credential is sealed before it is stored and
//! no route returns it.

use oss_blob::{BlobError, ByteRange, CreateOutcome, SliceSource};
use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::{
    MlService, bad, blob_failed, failed, fresh_id, now_millis, refuse, refuse_access_parameters,
};

/// What every connector shows as its `_index`.
const INDEX: &str = ".plugins-ml-connector";
/// The master key is 32 bytes in base64; a buffer a little larger reads it whole.
const KEY_BUFFER: usize = 128;
/// The fields an update may set.
const UPDATABLE: [&str; 8] = [
    "name",
    "description",
    "version",
    "protocol",
    "parameters",
    "credential",
    "actions",
    "client_config",
];

fn missing(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        // ALLOC: error path; one message.
        format!("Failed to find connector with the provided connector id: {id}"),
    )
}

/// An index write's answer, as stock's connector writes return it.
fn index_result(id: &str, result: &str) -> Value {
    json!({
        "_index": INDEX, "_id": id, "_version": 1, "result": result,
        "forced_refresh": true,
        "_shards": {"total": 1, "successful": 1, "failed": 0},
        "_seq_no": 0, "_primary_term": 1,
    })
}

impl MlService {
    fn master_key_blob(&self) -> String {
        // ALLOC: one key, once a node.
        format!("{}ml/master_key", self.base)
    }

    /// The stored master key, if one is written (1 GET).
    fn read_master_key(&self) -> Result<Option<String>, RestError> {
        let mut bytes: Vec<u8> = Vec::new();
        let mut buf = [0u8; KEY_BUFFER];
        let mut sink = |chunk: &[u8]| {
            if bytes.len().saturating_add(chunk.len()) <= KEY_BUFFER {
                bytes.extend_from_slice(chunk);
            }
            Ok(())
        };
        match self.store.get(
            &self.master_key_blob(),
            ByteRange::Full,
            &mut buf,
            &mut sink,
        ) {
            Ok(_) => Ok(String::from_utf8(bytes).ok().filter(|k| !k.is_empty())),
            Err(BlobError::NotFound) => Ok(None),
            Err(e) => Err(blob_failed(&e)),
        }
    }

    /// The cluster's master key, read once a node and remembered: made by
    /// conditional create on the first connector, so two nodes agree on one.
    fn master_key(&self) -> Result<&str, RestError> {
        if let Some(key) = self.master_key.get() {
            return Ok(key);
        }
        let key = match self.read_master_key()? {
            Some(key) => key,
            None => {
                let fresh = oss_ml::credentials::new_master_key()
                    .map_err(|e| refuse(500, "ml_store_failure", e))?;
                let mut buf = [0u8; KEY_BUFFER];
                let made = self
                    .store
                    .create_if_absent(
                        &self.master_key_blob(),
                        &mut SliceSource::new(fresh.as_bytes()),
                        &mut buf,
                    )
                    .map_err(|e| blob_failed(&e))?;
                if matches!(made, CreateOutcome::Created) {
                    fresh
                } else {
                    self.read_master_key()?.ok_or_else(|| {
                        refuse(500, "ml_store_failure", "the master key is unreadable")
                    })?
                }
            }
        };
        Ok(self.master_key.get_or_init(|| key))
    }

    fn connector_id(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("connector_id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a connector id is required"))
    }

    fn write_connector(
        &self,
        id: &str,
        connector: &Map<String, Value>,
        create: bool,
        marker: &mut [u8],
    ) -> Result<(), RestError> {
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = Value::Object(connector.clone()).to_string().into_bytes();
        self.set_of(DocKind::MlConnectors)?
            .put_counted(id, &bytes, create, marker)
            .map(|_| ())
            .map_err(|e| failed(&e))
    }

    /// `POST /connectors/_create`.
    pub(crate) fn create_connector(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let def = Self::body(req)?;
        let mut marker = self.marker(req)?;
        let id = self.insert_connector(def, &mut marker)?;
        resp.send(
            200,
            JSON,
            json!({"connector_id": id}).to_string().as_bytes(),
        )
    }

    /// Validates, seals and stores a connector definition under a new id.
    pub(super) fn insert_connector(
        &self,
        mut def: Map<String, Value>,
        marker: &mut [u8],
    ) -> Result<String, RestError> {
        refuse_access_parameters(&def)?;
        oss_ml::connector::validate(&def, &self.trusted).map_err(bad)?;
        oss_ml::connector::seal_definition(&mut def, self.master_key()?).map_err(bad)?;
        let t = now_millis();
        def.insert("created_time".into(), json!(t));
        def.insert("last_updated_time".into(), json!(t));
        let id = fresh_id()?;
        self.write_connector(&id, &def, true, marker)?;
        Ok(id)
    }

    /// `GET /connectors/{id}`: the definition without its credential.
    pub(crate) fn get_connector(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::connector_id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut def = self
            .stored_in(DocKind::MlConnectors, &id, &mut marker, &mut doc)?
            .ok_or_else(|| missing(&id))?;
        def.remove("credential");
        resp.send(200, JSON, Value::Object(def).to_string().as_bytes())
    }

    /// `PUT /connectors/{id}`: the fields of the body replace the stored ones.
    pub(crate) fn update_connector(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::connector_id(req)?;
        let body = Self::body(req)?;
        refuse_access_parameters(&body)?;
        let mut patch: Map<String, Value> = body
            .into_iter()
            .filter(|(k, v)| UPDATABLE.contains(&k.as_str()) && !v.is_null())
            .collect();
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut def = self
            .stored_in(DocKind::MlConnectors, &id, &mut marker, &mut doc)?
            .ok_or_else(|| missing(&id))?;
        // Only a credential in the body needs the key, so only it can cost a read.
        let key = if patch.contains_key("credential") {
            self.master_key()?
        } else {
            ""
        };
        oss_ml::connector::seal_definition(&mut patch, key).map_err(bad)?;
        def.extend(patch);
        oss_ml::connector::validate(&def, &self.trusted).map_err(bad)?;
        def.insert("last_updated_time".into(), json!(now_millis()));
        self.write_connector(&id, &def, false, &mut marker)?;
        resp.send(
            200,
            JSON,
            index_result(&id, "updated").to_string().as_bytes(),
        )
    }

    /// `DELETE /connectors/{id}`.
    pub(crate) fn delete_connector(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::connector_id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let using = self.models_using(&id, &mut marker, &mut doc)?;
        if !using.is_empty() {
            return Err(refuse(
                409,
                "status_exception",
                format!(
                    "{} models are still using this connector, please delete or update the \
                     models first: [{}]",
                    using.len(),
                    using.join(", ")
                ),
            ));
        }
        if !self
            .set_of(DocKind::MlConnectors)?
            .delete(&id, &mut marker)
            .map_err(|e| failed(&e))?
        {
            return Err(missing(&id));
        }
        resp.send(
            200,
            JSON,
            index_result(&id, "deleted").to_string().as_bytes(),
        )
    }

    /// `GET|POST /connectors/_search`.
    pub(crate) fn search_connectors(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.search_set(DocKind::MlConnectors, INDEX, req, resp)
    }
}
