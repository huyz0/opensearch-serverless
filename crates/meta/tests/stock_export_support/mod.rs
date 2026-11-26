//! Helpers shared by the publish and the fault tests of the export.
#![allow(dead_code)] // each test binary uses some of these

use std::sync::Arc;

use oss_blob::{Emulator, ExportStore};
use oss_meta::{
    ExportBase, ExportError, ExportIndex, ExportShard, ExportSnapshot, ExportTarget, ExportedFile,
    open_export_base, publish_snapshot,
};

pub const PREFIX: &str = "repo/";

pub fn file(blob: &str, physical: &str) -> ExportedFile {
    ExportedFile {
        name: blob.to_owned(),
        physical_name: physical.to_owned(),
        length: 40,
        checksum: "abc".to_owned(),
        written_by: "10.5.1".to_owned(),
    }
}

pub fn shards(tag: &str, n: usize) -> Vec<ExportShard> {
    (0..n)
        .map(|s| ExportShard {
            files: vec![
                file(&format!("__{tag}{s}a"), "_0.cfs"),
                file(&format!("__{tag}{s}b"), "segments_3"),
            ],
            index_version: 3,
        })
        .collect()
}

pub struct Take<'a> {
    pub name: &'a str,
    pub uuid: &'a str,
    pub index: &'a str,
    pub index_uuid: &'a str,
    pub shards: Vec<ExportShard>,
}

pub fn publish(
    store: &Arc<Emulator>,
    base: Option<ExportBase>,
    take: &Take<'_>,
) -> Result<oss_meta::Published, ExportError> {
    let export = ExportStore::new(store.clone(), PREFIX);
    let target = ExportTarget {
        store: &export,
        prefix: PREFIX,
    };
    let base = match base {
        Some(b) => b,
        None => open_export_base(&target)?,
    };
    let id = base
        .index_id(take.index)
        .map_or_else(|| format!("ID{}", take.index), str::to_owned);
    let settings = vec![("index.refresh_interval".to_owned(), "5s".to_owned())];
    let index = ExportIndex {
        name: take.index,
        id: &id,
        uuid: take.index_uuid,
        creation_date_ms: 1_700_000_000_000,
        mapping: Some(r#"{"properties":{"t":{"type":"text"}}}"#),
        settings: &settings,
        shards: &take.shards,
    };
    let indices = [index];
    let snap = ExportSnapshot {
        name: take.name,
        uuid: take.uuid,
        start_ms: 1_700_000_000_100,
        end_ms: 1_700_000_000_900,
        indices: &indices,
    };
    publish_snapshot(&target, base, &snap, &mut [0u8; 64])
}

pub fn first() -> Take<'static> {
    Take {
        name: "s1",
        uuid: "U1",
        index: "logs",
        index_uuid: "XU1",
        shards: shards("p", 2),
    }
}

pub fn store() -> Arc<Emulator> {
    Arc::new(Emulator::new(1 << 20, 1))
}
