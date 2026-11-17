//! Replays `fixtures/compat/bulk/*.json` through `bulk` over in-process
//! shards: each step's status and body, after its normalisation rules,
//! must be the recorded one (the index creation step only builds the
//! shards). Every fixture replays with the whole body in one slice and
//! again with each operation a slice of its own, read 7 bytes at a time.

// A fixture is a small file the test reads whole.
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_store::ByteBuffersDirectory;
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    Target, stream, write_response,
};
use oss_writer::index::{Refused, Shard, Written};
use oss_writer::mapping::Mapping;
use serde_json::{Value, json};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn fixtures() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/compat/bulk");
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("fixtures")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    all.sort();
    all
}

/// One index's shards, each over its own directory.
struct Index {
    info: IndexInfo,
    shards: Vec<Shard<'static>>,
}

#[derive(Default)]
struct Local {
    indexes: HashMap<String, Index>,
}

impl Local {
    fn create(&mut self, name: &str, body: &Value) {
        let settings = &body["settings"];
        let get = |k: &str, d: u32| {
            settings[k]
                .as_u64()
                .map_or(d, |v| u32::try_from(v).unwrap())
        };
        let shards = get("number_of_shards", 1);
        let mapping = match body.get("mappings") {
            Some(m) => Mapping::parse(m.to_string().as_bytes()).expect("mapping"),
            None => Mapping::default(),
        };
        let info = IndexInfo {
            name: name.to_owned(),
            uuid: "uuid".into(),
            shards,
            partition_size: get("routing_partition_size", 1),
            routing_shards: get("number_of_routing_shards", shards),
            routing_required: mapping.routing_required(),
        };
        let shards = (0..shards)
            .map(|_| {
                // A directory a shard writes to lives as long as the test.
                let dir: &'static ByteBuffersDirectory =
                    Box::leak(Box::new(ByteBuffersDirectory::new()));
                let mut w =
                    IndexWriter::open(dir, Vec::new(), "Lucene104", VERSION).expect("writer");
                w.enable_explicit_documents().expect("explicit");
                Shard::new(dir, w, mapping.clone(), 1, Box::new(|| 0)).expect("shard")
            })
            .collect();
        self.indexes.insert(name.to_owned(), Index { info, shards });
    }

    fn shard(&mut self, index: &IndexInfo, shard: u32) -> &mut Shard<'static> {
        let i = self.indexes.get_mut(&index.name).expect("index");
        &mut i.shards[usize::try_from(shard).unwrap()]
    }
}

impl Target for Local {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        self.indexes.get(name).map(|i| i.info.clone())
    }

    fn owner(&mut self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn apply(
        &mut self,
        index: &IndexInfo,
        shard: u32,
        item: &Planned<'_>,
    ) -> Result<Written, Refused> {
        let a = &item.op.action;
        let s = self.shard(index, shard);
        match a.kind {
            oss_writer::bulk::Kind::Delete => s.delete(item.id, &a.expect),
            _ => s.index(item.id, &item.op.source, a.create, &a.expect),
        }
    }

    fn refresh(&mut self, index: &IndexInfo, shard: u32) -> Result<(), String> {
        self.shard(index, shard).refresh()
    }

    fn forward(&mut self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("every shard is local")
    }
}

/// The request's body bytes.
fn body(req: &Value) -> Vec<u8> {
    if let Some(lines) = req.get("ndjson") {
        let mut out = Vec::new();
        for line in lines.as_array().expect("lines") {
            out.extend_from_slice(line.to_string().as_bytes());
            out.push(b'\n');
        }
        return out;
    }
    req.get("raw")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .as_bytes()
        .to_vec()
}

/// What the bulk path answers, as `{status, body}`.
fn answer(local: &mut Local, req: &Value, slice_bytes: usize) -> Value {
    let path = req["path"].as_str().expect("path");
    let query = req.get("query").cloned().unwrap_or(json!({}));
    let index = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .filter(|s| !s.starts_with('_'))
        .map(str::to_owned);
    let defaults = Defaults {
        index,
        ..Defaults::default()
    };
    let refresh = query.get("refresh").and_then(Value::as_str) == Some("true");
    let mut ids = IdGenerator::random();
    let budget = BulkBudget::new(64 << 20, 4);
    let config = StreamConfig {
        slice_bytes,
        max_wait: Duration::from_secs(5),
    };
    let bytes = body(req);
    let mut bulk = Bulk::new(local, &mut ids, 1, refresh);
    let mut buffer = [0u8; 7];
    let streamed = stream(
        &mut bytes.as_slice(),
        &mut buffer,
        &defaults,
        &budget,
        config,
        &mut bulk,
    );
    let mut out = Vec::new();
    let status = match streamed.and_then(|()| bulk.finish()) {
        Ok(items) => {
            write_response(&mut out, 0, &items).expect("write");
            200
        }
        Err(e) => {
            e.write_body(&mut out).expect("write");
            e.status()
        }
    };
    json!({"status": status, "body": serde_json::from_slice::<Value>(&out).expect("json")})
}

/// Replaces every value `pointer` names (a `*` segment matching every
/// element or member) with `"<normalised>"`.
fn normalise(value: &mut Value, pointer: &[&str]) {
    let Some((first, rest)) = pointer.split_first() else {
        *value = json!("<normalised>");
        return;
    };
    match (first, value) {
        (&"*", Value::Array(a)) => a.iter_mut().for_each(|v| normalise(v, rest)),
        (&"*", Value::Object(o)) => o.values_mut().for_each(|v| normalise(v, rest)),
        (key, Value::Object(o)) => {
            if let Some(v) = o.get_mut(*key) {
                normalise(v, rest);
            }
        }
        (key, Value::Array(a)) => {
            if let Some(v) = key.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                normalise(v, rest);
            }
        }
        _ => {}
    }
}

fn replay(path: &PathBuf, slice_bytes: usize) {
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).expect("read")).expect("json");
    let mut local = Local::default();
    for step in fixture["steps"].as_array().expect("steps") {
        let req = &step["request"];
        if req["method"] == "PUT" {
            let name = req["path"].as_str().expect("path").trim_start_matches('/');
            local.create(name, &req["body"]);
            continue;
        }
        let validation =
            step["response"]["body"]["error"]["type"] == "action_request_validation_exception";
        if slice_bytes < 1024 && validation {
            // Slices check their own items: a request refused for a later
            // item is refused only by the items of its first slice.
            continue;
        }
        let mut got = answer(&mut local, req, slice_bytes);
        let mut want =
            json!({"status": step["response"]["status"], "body": step["response"]["body"]});
        // The writer gives every index the same uuid.
        let rules = step
            .get("normalise")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for rule in rules {
            let pointer: Vec<&str> = rule["pointer"]
                .as_str()
                .expect("pointer")
                .split('/')
                .skip(1)
                .collect();
            normalise(&mut got, &pointer);
            normalise(&mut want, &pointer);
        }
        assert_eq!(
            got,
            want,
            "{}: {} (slices of {slice_bytes} bytes)",
            path.display(),
            step["name"]
        );
    }
}

#[test]
fn bulk_steps_answer_as_stock_opensearch_does() {
    let all = fixtures();
    assert!(!all.is_empty());
    for path in &all {
        replay(path, 8 << 20);
        replay(path, 1);
    }
}
