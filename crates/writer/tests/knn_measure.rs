//! Recall against exact search, graph-build memory and search time of a
//! shard's `knn_vector` field, at a size the environment names. Not a
//! pass/fail test: it prints the numbers `docs/knn-measurements.md` records,
//! and is ignored so `cargo test` does not run it.
//!
//! ```text
//! KNN_N=100000 KNN_DIM=128 KNN_SPACE=l2 KNN_DATA=clustered \
//!   cargo test -p oss-writer --release --test knn_measure -- --ignored --nocapture
//! ```
//!
//! Its own binary, because the counting allocator is process-wide.

// ARITH: measurement code over small counts; an overflow panics, which fails it.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::time::Instant;

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::knn_search::{KnnTarget, knn_top_hits};
use lucene_store::ByteBuffersDirectory;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use oss_writer::index::{Expect, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Roughly normal: the sum of four uniforms, centred and scaled.
    fn gauss(&mut self) -> f32 {
        (self.unit() + self.unit() + self.unit() + self.unit() - 2.0) * 1.7
    }
}

fn vectors(n: usize, dim: usize, clustered: bool, rng: &mut Rng, centres: &[Vec<f32>]) -> Vec<f32> {
    let mut out = Vec::with_capacity(n * dim);
    for _ in 0..n {
        if clustered {
            let c = &centres[(rng.next() % centres.len() as u64) as usize];
            out.extend(c.iter().map(|x| x + 0.3 * rng.gauss()));
        } else {
            out.extend((0..dim).map(|_| rng.gauss()));
        }
    }
    out
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The plugin's score of a pair, the order the engine ranks by.
fn score(space: &str, a: &[f32], b: &[f32]) -> f32 {
    match space {
        "l2" => {
            let d: f32 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum();
            1.0 / (1.0 + d)
        }
        "cosinesimil" => (1.0 + dot(a, b) / (dot(a, a).sqrt() * dot(b, b).sqrt())) / 2.0,
        _ => {
            let d = dot(a, b);
            if d < 0.0 { 1.0 / (1.0 - d) } else { d + 1.0 }
        }
    }
}

fn rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[test]
#[ignore = "a measurement, run by hand"]
fn measure() {
    let n: usize = env("KNN_N", 100_000);
    let dim: usize = env("KNN_DIM", 128);
    let space: String = env("KNN_SPACE", "l2".to_owned());
    let clustered = env("KNN_DATA", "clustered".to_owned()) == "clustered";
    let batch: usize = env("KNN_BATCH", n);
    let queries: usize = env("KNN_QUERIES", 100);
    let m: usize = env("KNN_M", 16);
    let ef: usize = env("KNN_EF", 100);

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let centres: Vec<Vec<f32>> = (0..100)
        .map(|_| (0..dim).map(|_| 2.0 * rng.gauss()).collect())
        .collect();
    let data = vectors(n, dim, clustered, &mut rng, &centres);
    let qs = vectors(queries, dim, clustered, &mut rng, &centres);

    let mapping = format!(
        r#"{{"properties":{{"v":{{"type":"knn_vector","dimension":{dim},"method":{{"name":"hnsw","engine":"lucene","space_type":"{space}","parameters":{{"m":{m},"ef_construction":{ef}}}}}}}}}}}"#
    );
    let mapping = Mapping::parse_with(
        mapping.as_bytes(),
        ParseOptions {
            knn: Some(true),
            ..ParseOptions::default()
        },
    )
    .expect("mapping");
    let dir = ByteBuffersDirectory::new();
    let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
    w.enable_explicit_documents().expect("explicit");
    let mut shard = Shard::new(&dir, w, mapping, 1, Box::new(|| 0)).expect("shard");

    let started = Instant::now();
    let ingest = PeakWindow::start(&ALLOC);
    let mut refresh_peak = 0usize;
    let mut buffered = 0usize;
    let mut text = String::new();
    for (i, v) in data.chunks(dim).enumerate() {
        text.clear();
        text.push_str(r#"{"v":["#);
        for (j, x) in v.iter().enumerate() {
            if j > 0 {
                text.push(',');
            }
            text.push_str(&x.to_string());
        }
        text.push_str("]}");
        shard
            .index(&format!("d{i}"), text.as_bytes(), false, &Expect::default())
            .expect("index");
        if (i + 1) % batch == 0 || i + 1 == n {
            buffered = buffered.max(shard.indexing_buffer_bytes());
            let window = PeakWindow::start(&ALLOC);
            let before = window.live_bytes();
            shard.refresh().expect("refresh");
            refresh_peak = refresh_peak.max(window.peak_bytes());
            let _ = before;
        }
    }
    let index_secs = started.elapsed().as_secs_f64();
    drop(shard);
    let (ingest_peak, ingest_live) = (ingest.peak_bytes(), ingest.live_bytes());

    let reader = DirectoryReader::open(&dir).expect("open");
    let segments = reader.open_segments().expect("segments");
    let open = segments.as_open_segments();
    println!(
        "config: n={n} dim={dim} space={space} data={} m={m} ef_construction={ef} batch={batch} segments={}",
        if clustered { "clustered" } else { "uniform" },
        open.len()
    );
    println!(
        "build: index+refresh {index_secs:.1}s, buffered vectors+docs peak {} MiB (raw vectors {} MiB), refresh peak above its start {} MiB, process VmHWM {} MiB",
        buffered >> 20,
        (n * dim * 4) >> 20,
        refresh_peak >> 20,
        rss_kib() >> 10
    );
    println!(
        "ingest: peak above its start {} MiB, held after (the segments in the directory) {} MiB, transient {} MiB",
        ingest_peak >> 20,
        ingest_live >> 20,
        ingest_peak.saturating_sub(ingest_live) >> 20
    );

    for k in [10usize, 100] {
        let mut hit = 0usize;
        let mut total_us = 0u128;
        for q in qs.chunks(dim) {
            let t = Instant::now();
            let got =
                knn_top_hits(&open, "v", &KnnTarget::Float(q.to_vec()), k, k, None).expect("knn");
            total_us += t.elapsed().as_micros();
            let mut exact: Vec<(f32, usize)> = data
                .chunks(dim)
                .enumerate()
                .map(|(i, v)| (score(&space, q, v), i))
                .collect();
            exact.sort_by(|a, b| b.0.total_cmp(&a.0));
            let truth: std::collections::HashSet<usize> =
                exact.iter().take(k).map(|(_, i)| *i).collect();
            hit += got
                .iter()
                .filter(|h| truth.contains(&(h.doc_id as usize)))
                .count();
        }
        println!(
            "recall@{k}: {:.4} over {queries} queries, mean search {:.2} ms",
            hit as f64 / (k * queries) as f64,
            total_us as f64 / queries as f64 / 1000.0
        );
    }
}
