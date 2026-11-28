//! A local model's file: `models/meta` records the model, `upload_chunk` streams
//! each piece of its file into one blob, so no request holds more than one body buffer.

use oss_blob::{BlobError, BlobResult, ChunkSource};
use oss_rest::{Body, JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::{MlService, bad, fresh_id, now_millis, refuse};

/// The most chunks a model's file may be cut into.
const MAX_CHUNKS: u64 = 4096;
/// The most bytes one chunk may carry.
const MAX_CHUNK_BYTES: u64 = 64 << 20;

/// A request body as a blob's source: the declared length, then each body chunk.
struct BodySource<'a, 'b> {
    body: &'b mut Body<'a>,
    len: u64,
    /// What the last body chunk had left after the caller's buffer filled.
    spare: Vec<u8>,
    at: usize,
}

impl ChunkSource for BodySource<'_, '_> {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        if self.at >= self.spare.len() {
            self.spare.clear();
            self.at = 0;
            match self.body.next_chunk() {
                Ok(Some(chunk)) => self.spare.extend_from_slice(chunk),
                Ok(None) => return Ok(0),
                Err(_) => return Err(BlobError::Stream("request body failed")),
            }
        }
        let rest = self.spare.get(self.at..).unwrap_or_default();
        let n = rest.len().min(buf.len());
        if let (Some(to), Some(from)) = (buf.get_mut(..n), rest.get(..n)) {
            to.copy_from_slice(from);
        }
        // ARITH: n is at most the bytes left in `spare`.
        #[allow(clippy::arithmetic_side_effects)]
        {
            self.at += n;
        }
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        Err(BlobError::Stream("a request body cannot restart"))
    }
}

impl MlService {
    /// The key of chunk `n` of model `id`.
    fn chunk_key(&self, id: &str, n: u64) -> String {
        // ALLOC: one key a chunk.
        format!("{}/ml/artifacts/{id}/{n:08}", self.base)
    }

    /// `POST /models/meta`: a local model whose file arrives by `upload_chunk`.
    pub(crate) fn register_meta(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::body(req)?;
        let text = |k: &str| {
            body.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let name = text("name").ok_or_else(|| bad("model name is null"))?;
        let function =
            text("function_name").map_or_else(|| "TEXT_EMBEDDING".to_owned(), str::to_uppercase);
        let total = body
            .get("total_chunks")
            .and_then(Value::as_u64)
            .filter(|n| (1..=MAX_CHUNKS).contains(n))
            .ok_or_else(|| bad("total_chunks must be between 1 and 4096"))?;
        let mut marker = self.marker(req)?;
        let id = fresh_id()?;
        let t = now_millis();
        let mut model = Map::new();
        model.insert("name".into(), json!(name));
        model.insert("model_id".into(), json!(id));
        model.insert("algorithm".into(), json!(function));
        model.insert("model_version".into(), json!("1"));
        model.insert("model_state".into(), json!("REGISTERING"));
        model.insert("total_chunks".into(), json!(total));
        model.insert("chunks_uploaded".into(), json!([]));
        for key in ["model_format", "model_config", "model_content_hash_value"] {
            if let Some(v) = body.get(key) {
                model.insert(key.into(), v.clone());
            }
        }
        model.insert("created_time".into(), json!(t));
        model.insert("last_updated_time".into(), json!(t));
        model.insert("is_hidden".into(), json!(false));
        self.write_model(&id, &model, true, &mut marker)?;
        resp.send(
            200,
            JSON,
            json!({"model_id": id, "status": "CREATED"})
                .to_string()
                .as_bytes(),
        )
    }

    /// `POST /models/{id}/upload_chunk/{chunk_number}`: the body is one piece of the file.
    pub(crate) fn upload_chunk(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let number = req
            .path_param("chunk_number")
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or_else(|| bad("chunk_number must be a number"))?;
        let len = req
            .body()
            .declared_length()
            .filter(|l| (1..=MAX_CHUNK_BYTES).contains(l))
            .ok_or_else(|| {
                refuse(
                    400,
                    "illegal_argument_exception",
                    "a chunk needs a Content-Length of 1 byte to 64 MiB",
                )
            })?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut model = self.stored_model(&id, &mut marker, &mut doc)?;
        let total = model
            .get("total_chunks")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("the model was not registered for chunked upload"))?;
        if number >= total {
            return Err(bad(format!(
                "chunk {number} is out of range for {total} chunks"
            )));
        }
        let max = req.body().max_chunk();
        req.body().reserve(max)?;
        let mut buf = vec![0u8; self.buffer_bytes];
        let mut source = BodySource {
            body: req.body(),
            len,
            // ALLOC: at most one body buffer, reserved above.
            spare: Vec::with_capacity(max),
            at: 0,
        };
        self.store
            .put(&self.chunk_key(&id, number), &mut source, &mut buf)
            .map_err(|e| refuse(500, "ml_store_failure", e.to_string()))?;
        let mut done: Vec<u64> = model
            .get("chunks_uploaded")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        if !done.contains(&number) {
            done.push(number);
        }
        let complete = u64::try_from(done.len()).is_ok_and(|n| n == total);
        model.insert("chunks_uploaded".into(), json!(done));
        if complete {
            model.insert("model_state".into(), json!("REGISTERED"));
        }
        model.insert("last_updated_time".into(), json!(now_millis()));
        self.write_model(&id, &model, false, &mut marker)?;
        resp.send(
            200,
            JSON,
            json!({"status": "Uploaded"}).to_string().as_bytes(),
        )
    }
}
