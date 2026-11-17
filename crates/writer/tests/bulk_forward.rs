//! A group whose shard another node owns is forwarded once, and its
//! outcomes take their places in request order among the local ones.

use std::time::Duration;

use oss_writer::bulk::{
    Bulk, BulkBudget, Defaults, IdGenerator, IndexInfo, Item, ItemResult, Owner, Planned,
    StreamConfig, Target, stream,
};
use oss_writer::index::{Outcome, Refused, Written};

/// Four shards: the even ones live here, the odd ones on `n2`.
#[derive(Default)]
struct Split {
    local: Vec<String>,
    forwarded: Vec<(String, u32, Vec<String>)>,
}

impl Target for Split {
    fn index(&mut self, name: &str) -> Option<IndexInfo> {
        Some(IndexInfo {
            name: name.into(),
            uuid: "u".into(),
            shards: 4,
            partition_size: 1,
            routing_shards: 4,
            routing_required: false,
        })
    }

    fn owner(&mut self, _: &IndexInfo, shard: u32) -> Owner {
        if shard.is_multiple_of(2) {
            Owner::Local
        } else {
            Owner::Node("n2".into())
        }
    }

    fn apply(&mut self, _: &IndexInfo, shard: u32, item: &Planned<'_>) -> Result<Written, Refused> {
        self.local.push(item.id.to_owned());
        Ok(Written {
            outcome: Outcome::Created,
            version: 1,
            seq_no: i64::from(shard),
            primary_term: 1,
        })
    }

    fn refresh(&mut self, _: &IndexInfo, _: u32) -> Result<(), String> {
        Ok(())
    }

    fn forward(
        &mut self,
        node: &str,
        _: &IndexInfo,
        shard: u32,
        group: &[Planned<'_>],
    ) -> Vec<ItemResult> {
        self.forwarded.push((
            node.to_owned(),
            shard,
            group.iter().map(|p| p.id.to_owned()).collect(),
        ));
        group
            .iter()
            .map(|_| ItemResult::Written {
                written: Written {
                    outcome: Outcome::Updated,
                    version: 9,
                    seq_no: i64::from(shard).saturating_add(100),
                    primary_term: 2,
                },
                forced_refresh: false,
            })
            .collect()
    }
}

fn run(body: &str, slice_bytes: usize) -> (Split, Vec<Item>) {
    let defaults = Defaults {
        index: Some("x".into()),
        ..Defaults::default()
    };
    let budget = BulkBudget::new(1 << 20, 2);
    let config = StreamConfig {
        slice_bytes,
        max_wait: Duration::from_secs(5),
    };
    let mut target = Split::default();
    let mut ids = IdGenerator::new(0, [0; 6]);
    let mut buffer = [0u8; 16];
    let mut bulk = Bulk::new(&mut target, &mut ids, 1, false);
    stream(
        &mut body.as_bytes(),
        &mut buffer,
        &defaults,
        &budget,
        config,
        &mut bulk,
    )
    .unwrap();
    let items = bulk.finish().unwrap();
    (target, items)
}

fn body(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{{\"n\":1}}\n"))
        .collect()
}

#[test]
fn a_foreign_group_is_forwarded_once_and_answers_in_request_order() {
    // Ids "0".."7" route to shards 1,2,1,1,1,0,2,0 over four shards of
    // murmur3 modulo four; the group of shard 1 is not contiguous.
    let ids: Vec<String> = (0..16).map(|i| i.to_string()).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (target, items) = run(&body(&refs), 1 << 20);
    assert_eq!(items.len(), 16);
    // One forward call a foreign shard, each holding its items in request order.
    let mut shards: Vec<u32> = target.forwarded.iter().map(|f| f.1).collect();
    shards.sort_unstable();
    let before = shards.len();
    shards.dedup();
    assert_eq!(shards.len(), before, "a shard was forwarded twice");
    assert!(shards.iter().all(|s| !s.is_multiple_of(2)));
    let sent: usize = target.forwarded.iter().map(|f| f.2.len()).sum();
    assert_eq!(sent + target.local.len(), 16);
    assert!(sent > 2 && !target.local.is_empty());
    assert!(target.forwarded.iter().any(|f| f.2.len() > 1));
    for (node, _, group) in &target.forwarded {
        assert_eq!(node, "n2");
        let positions: Vec<usize> = group
            .iter()
            .map(|g| ids.iter().position(|i| i == g).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
    }
    // Every item is answered where it was asked, by whoever applied it.
    for (i, item) in items.iter().enumerate() {
        assert_eq!(item.id, ids[i]);
        let ItemResult::Written { written, .. } = &item.result else {
            panic!("item {i} failed");
        };
        let foreign = target.forwarded.iter().any(|f| f.2.contains(&ids[i]));
        assert_eq!(written.version == 9, foreign, "item {i}");
    }
}

#[test]
fn each_slice_forwards_its_own_groups() {
    // One-byte slices: every operation is a slice, so each forward carries one item.
    let (target, items) = run(&body(&["0", "1", "2", "3", "4", "5"]), 1);
    assert_eq!(items.len(), 6);
    assert!(target.forwarded.iter().all(|f| f.2.len() == 1));
    assert_eq!(target.forwarded.len() + target.local.len(), 6);
}
