//! docs/specs/coordinator-by-query: a delete- or update-by-query walks one
//! shard at a time.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::cell::{Cell, RefCell};

use oss_coordinator::docs::by_query::{Applied, Change, Hit, Source, Walk, walk};

/// A shard is its hits' (id, seq_no), ascending.
struct Fake {
    shards: Vec<(u32, Vec<(String, i64)>)>,
    asks: RefCell<Vec<String>>,
    admitted: RefCell<Vec<u64>>,
    open: Cell<u32>,
    fail_admit: bool,
    fail_page: bool,
}

struct Permit<'a>(&'a Cell<u32>);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl Fake {
    fn new(shards: Vec<(u32, Vec<(&str, i64)>)>) -> Self {
        Self {
            shards: shards
                .into_iter()
                .map(|(s, h)| (s, h.into_iter().map(|(i, n)| (i.to_owned(), n)).collect()))
                .collect(),
            asks: RefCell::default(),
            admitted: RefCell::default(),
            open: Cell::new(0),
            fail_admit: false,
            fail_page: false,
        }
    }
    fn hits(&self, shard: u32) -> &[(String, i64)] {
        &self
            .shards
            .iter()
            .find(|(s, _)| *s == shard)
            .expect("shard")
            .1
    }
}

impl Source for Fake {
    type Error = &'static str;
    type Permit<'a>
        = Permit<'a>
    where
        Self: 'a;

    fn newest(&self, shard: u32) -> Result<Option<i64>, Self::Error> {
        self.asks.borrow_mut().push(format!("newest {shard}"));
        Ok(self.hits(shard).last().map(|(_, n)| *n))
    }

    fn page(
        &self,
        shard: u32,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, Self::Error> {
        self.asks.borrow_mut().push(format!(
            "page {shard} <={up_to} after {after:?} size {size}"
        ));
        if self.fail_page {
            return Err("page failed");
        }
        Ok(self
            .hits(shard)
            .iter()
            .filter(|(_, n)| *n <= up_to && after.is_none_or(|a| *n > a))
            .take(size)
            .map(|(id, n)| Hit {
                id: id.clone(),
                seq_no: *n,
                primary_term: 1,
            })
            .collect())
    }

    fn admit(&self, bytes: u64) -> Result<Self::Permit<'_>, Self::Error> {
        if self.fail_admit {
            return Err("refused");
        }
        self.admitted.borrow_mut().push(bytes);
        self.open.set(self.open.get() + 1);
        Ok(Permit(&self.open))
    }
}

/// Records what it was asked and answers from a script by id.
struct Recorder<'a> {
    seen: Vec<(String, bool)>,
    conflict_on: &'a [&'a str],
    noop_on: &'a [&'a str],
    open_during: Vec<u32>,
    open: &'a Cell<u32>,
    fail_on: Option<&'a str>,
}

impl Change for Recorder<'_> {
    type Error = &'static str;
    fn apply(&mut self, hit: &Hit, refresh: bool) -> Result<Applied, Self::Error> {
        self.seen.push((hit.id.clone(), refresh));
        self.open_during.push(self.open.get());
        if self.fail_on == Some(hit.id.as_str()) {
            return Err("change failed");
        }
        if self.conflict_on.contains(&hit.id.as_str()) {
            return Ok(Applied::Conflict(format!("conflict on {}", hit.id)));
        }
        Ok(if self.noop_on.contains(&hit.id.as_str()) {
            Applied::Noop
        } else {
            Applied::Changed
        })
    }
}

fn recorder<'a>(fake: &'a Fake) -> Recorder<'a> {
    Recorder {
        seen: Vec::new(),
        conflict_on: &[],
        noop_on: &[],
        open_during: Vec::new(),
        open: &fake.open,
        fail_on: None,
    }
}

#[test]
fn shards_are_walked_in_order_and_an_empty_one_is_not_paged() {
    let fake = Fake::new(vec![
        (3, vec![]),
        (1, vec![("a", 5), ("b", 6)]),
        (2, vec![("c", 1)]),
    ]);
    let mut change = recorder(&fake);
    let out = walk(&fake, &[3, 1, 2], 100, false, false, &mut change).expect("walk");
    assert_eq!(
        *fake.asks.borrow(),
        [
            "newest 3",
            "newest 1",
            "page 1 <=6 after None size 100",
            "newest 2",
            "page 2 <=1 after None size 98",
        ]
    );
    let ids: Vec<&str> = change.seen.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!((out.visited, out.changed, out.batches), (3, 3, 2));
}

#[test]
fn pages_follow_the_last_number_and_are_bounded_by_the_start() {
    let hits: Vec<(String, i64)> = (1..=2500).map(|n| (format!("d{n}"), n)).collect();
    let mut fake = Fake::new(vec![]);
    fake.shards.push((0, hits));
    let mut change = recorder(&fake);
    let out = walk(&fake, &[0], 10_000, false, false, &mut change).expect("walk");
    assert_eq!(
        *fake.asks.borrow(),
        [
            "newest 0",
            "page 0 <=2500 after None size 1000",
            "page 0 <=2500 after Some(1000) size 1000",
            "page 0 <=2500 after Some(2000) size 1000",
        ]
    );
    assert_eq!((out.visited, out.batches), (2500, 3));
}

#[test]
fn the_walk_stops_at_max_docs_across_shards() {
    let fake = Fake::new(vec![
        (0, vec![("a", 1), ("b", 2), ("c", 3)]),
        (1, vec![("d", 1)]),
    ]);
    let mut change = recorder(&fake);
    let out = walk(&fake, &[0, 1], 2, false, false, &mut change).expect("walk");
    assert_eq!(out.visited, 2);
    assert_eq!(
        *fake.asks.borrow(),
        ["newest 0", "page 0 <=3 after None size 2"]
    );
    assert_eq!(change.seen.len(), 2);
}

#[test]
fn changes_noops_conflicts_and_batches_are_counted() {
    let fake = Fake::new(vec![(0, vec![("a", 1), ("b", 2), ("c", 3), ("d", 4)])]);
    let mut change = recorder(&fake);
    change.noop_on = &["b"];
    change.conflict_on = &["c"];
    let out = walk(&fake, &[0], 100, false, false, &mut change).expect("walk");
    assert_eq!(
        out,
        Walk {
            visited: 4,
            changed: 2,
            noops: 1,
            conflicts: 1,
            aborted_on: None,
            conflict: None,
            batches: 1,
        }
    );
}

#[test]
fn aborting_on_conflict_ends_at_the_hit_with_its_message() {
    let fake = Fake::new(vec![(0, vec![("a", 1), ("b", 2), ("c", 3)])]);
    let mut change = recorder(&fake);
    change.conflict_on = &["b"];
    let out = walk(&fake, &[0], 100, false, true, &mut change).expect("walk");
    assert_eq!(
        out,
        Walk {
            visited: 2,
            changed: 1,
            noops: 0,
            conflicts: 1,
            aborted_on: Some("b".into()),
            conflict: Some("conflict on b".into()),
            batches: 1,
        }
    );
    assert_eq!(change.seen.len(), 2);
}

#[test]
fn a_refresh_rides_only_the_last_hit_of_a_shards_last_page() {
    let fake = Fake::new(vec![
        (0, vec![("a", 1), ("b", 2)]),
        (1, vec![("c", 1), ("d", 2)]),
    ]);
    let mut change = recorder(&fake);
    walk(&fake, &[0, 1], 100, true, false, &mut change).expect("walk");
    assert_eq!(
        change.seen,
        [
            ("a".to_owned(), false),
            ("b".to_owned(), true),
            ("c".to_owned(), false),
            ("d".to_owned(), true),
        ]
    );
    let mut change = recorder(&fake);
    walk(&fake, &[0, 1], 100, false, false, &mut change).expect("walk");
    assert!(change.seen.iter().all(|(_, refresh)| !refresh));
}

#[test]
fn a_page_is_admitted_by_its_bytes_and_held_while_it_is_changed() {
    let fake = Fake::new(vec![(0, vec![("ab", 1), ("cdef", 2)])]);
    let mut change = recorder(&fake);
    walk(&fake, &[0], 100, false, false, &mut change).expect("walk");
    assert_eq!(*fake.admitted.borrow(), [64 + 2 + 64 + 4]);
    assert_eq!(change.open_during, [1, 1]);
    assert_eq!(fake.open.get(), 0);
}

#[test]
fn a_refused_admission_a_failed_page_and_a_failed_change_end_the_walk() {
    let mut fake = Fake::new(vec![(0, vec![("a", 1)])]);
    fake.fail_admit = true;
    let mut change = recorder(&fake);
    assert_eq!(
        walk(&fake, &[0], 100, false, false, &mut change).err(),
        Some("refused")
    );
    assert!(change.seen.is_empty());
    let mut fake = Fake::new(vec![(0, vec![("a", 1)])]);
    fake.fail_page = true;
    let mut change = recorder(&fake);
    assert_eq!(
        walk(&fake, &[0], 100, false, false, &mut change).err(),
        Some("page failed")
    );
    let fake = Fake::new(vec![(0, vec![("a", 1), ("b", 2)])]);
    let mut change = recorder(&fake);
    change.fail_on = Some("a");
    assert_eq!(
        walk(&fake, &[0], 100, false, false, &mut change).err(),
        Some("change failed")
    );
    assert_eq!(fake.open.get(), 0);
}

#[test]
fn reaching_max_docs_makes_that_page_the_last_so_the_refresh_rides_it() {
    let fake = Fake::new(vec![(0, vec![("a", 1), ("b", 2), ("c", 3)])]);
    let mut change = recorder(&fake);
    walk(&fake, &[0], 2, true, false, &mut change).expect("walk");
    assert_eq!(
        change.seen,
        [("a".to_owned(), false), ("b".to_owned(), true)]
    );
}
