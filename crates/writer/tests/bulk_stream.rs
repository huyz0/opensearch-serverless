//! A bulk body streams: 64 MiB of operations go through `bulk::stream` a
//! 1 MiB slice at a time and the writer's peak heap stays far below the
//! body. Its own binary, because the counting allocator is process-wide.

// ARITH: the body generator counts bytes of its own fixed 64 MiB; an overflow would be a test bug, and debug builds panic on it.
#![allow(clippy::arithmetic_side_effects)]

use std::io::Read;
use std::time::Duration;

use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, ItemResult, Owner, Planned, StreamConfig,
    Target, stream,
};
use oss_writer::index::{Outcome, Refused, Written};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const BODY: usize = 64 << 20;
const DOC: usize = 16 << 10;
/// Heap a request may hold beyond one slice: the read buffer, the items'
/// answers (a few hundred bytes each, 4,096 of them), the tokenizer.
const SLICE: usize = 1 << 20;
const LIMIT: usize = 6 << 20;

/// A body made up as it is read: an index action and a source of
/// `DOC` bytes, over and over, until `BODY` bytes have been given out.
struct Body {
    sent: usize,
    pending: Vec<u8>,
    at: usize,
    n: usize,
}

impl Read for Body {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at == self.pending.len() {
            if self.sent >= BODY {
                return Ok(0);
            }
            self.pending.clear();
            self.at = 0;
            self.n += 1;
            self.pending.extend_from_slice(
                format!("{{\"index\":{{\"_id\":\"{}\"}}}}\n", self.n).as_bytes(),
            );
            self.pending.extend_from_slice(b"{\"t\":\"");
            self.pending.resize(self.pending.len() + DOC, b'x');
            self.pending.extend_from_slice(b"\"}\n");
            self.sent += self.pending.len();
        }
        let n = out.len().min(self.pending.len() - self.at);
        out[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Counts what reaches the shard and keeps none of it.
#[derive(Default)]
struct Count {
    items: usize,
    bytes: usize,
}

impl Target for Count {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        Some(IndexInfo {
            name: name.into(),
            uuid: "u".into(),
            shards: 1,
            partition_size: 1,
            routing_shards: 1,
            routing_required: false,
            write_block: Default::default(),
            default_pipeline: None,
            final_pipeline: None,
        })
    }

    fn owner(&mut self, _: &IndexInfo, _: u32) -> Owner {
        Owner::Local
    }

    fn apply(&mut self, _: &IndexInfo, _: u32, item: &Planned<'_>) -> Result<Written, Refused> {
        self.items += 1;
        self.bytes += item.op.source.len();
        Ok(Written {
            outcome: Outcome::Created,
            version: 1,
            seq_no: i64::try_from(self.items).unwrap(),
            primary_term: 1,
        })
    }

    fn refresh(&mut self, _: &IndexInfo, _: u32) -> Result<(), String> {
        Ok(())
    }

    fn forward(&mut self, _: &str, _: &IndexInfo, _: u32, _: &[Planned<'_>]) -> Vec<ItemResult> {
        unreachable!("one local shard")
    }
}

#[test]
fn a_64_mib_bulk_streams_in_slices() {
    let defaults = Defaults {
        index: Some("x".into()),
        ..Defaults::default()
    };
    let budget = BulkBudget::new(8 << 20, 2);
    let config = StreamConfig {
        slice_bytes: SLICE,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Count::default();
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = vec![0u8; 64 << 10];
    let mut body = Body {
        sent: 0,
        pending: Vec::new(),
        at: 0,
        n: 0,
    };
    let window = PeakWindow::start(&ALLOC);
    let mut bulk = Bulk::new(&mut target, &mut ids, 1, false);
    stream(
        &mut body,
        &mut buffer,
        &defaults,
        &budget,
        config,
        &mut bulk,
    )
    .expect("stream");
    let items = bulk.finish().expect("finish");
    let peak = window
        .check_below(LIMIT)
        .expect("the bulk held in proportion to its body");
    assert!(peak > 0);
    assert_eq!(items.len(), body.n);
    assert_eq!(target.items, body.n);
    assert!(target.bytes > BODY - body.n * 64);
    // Every grant went back.
    assert_eq!(budget.in_use(), (0, 0));
}
