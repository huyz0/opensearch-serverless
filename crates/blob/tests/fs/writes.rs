//! PUT and conditional create: whole objects, never part-written.

use std::sync::{Arc, Barrier};
use std::thread;

use oss_blob::{
    BlobError, BlobResult, BlobStore, ByteRange, ChunkSource, CreateOutcome, S3Op, SliceSource,
};

use super::{assert_counts, get_all, list_all, pattern, put, store};

/// A source that declares one length and yields another.
struct Lying {
    declared: u64,
    actual: Vec<u8>,
    at: usize,
}

impl ChunkSource for Lying {
    fn len(&self) -> u64 {
        self.declared
    }
    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let rest = &self.actual[self.at..];
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        self.at = self.at.saturating_add(n);
        Ok(n)
    }
    fn rewind(&mut self) -> BlobResult<()> {
        self.at = 0;
        Ok(())
    }
}

/// Every file under the store's root, staged ones included.
fn files_on_disk(root: &std::path::Path) -> usize {
    fn walk(dir: &std::path::Path, n: &mut usize) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, n);
            } else {
                *n = n.saturating_add(1);
            }
        }
    }
    let mut n = 0;
    walk(root, &mut n);
    n
}

#[test]
fn a_put_replaces_the_object_whole() {
    let (s, _dir) = store("fs-replace");
    put(&s, "k", &pattern(3000));
    put(&s, "k", b"short");
    assert_eq!(get_all(&s, "k", ByteRange::Full).unwrap(), b"short");
    assert_eq!(files_on_disk(s.root()), 1);
}

#[test]
fn a_put_with_a_bad_body_leaves_the_old_object_and_no_staged_file() {
    let (s, _dir) = store("fs-bad-body");
    put(&s, "a/k", b"old");
    for (declared, actual) in [(10, 5), (5, 10), (0, 1)] {
        let mut body = Lying {
            declared,
            actual: pattern(actual),
            at: 0,
        };
        let r = s.put("a/k", &mut body, &mut [0; 4]);
        assert!(
            matches!(r, Err(BlobError::InvalidArgument(_))),
            "{declared}/{actual}: {r:?}"
        );
        body.rewind().unwrap();
        let r = s.create_if_absent("a/new", &mut body, &mut [0; 4]);
        assert!(
            matches!(r, Err(BlobError::InvalidArgument(_))),
            "{declared}/{actual}: {r:?}"
        );
    }
    assert_eq!(get_all(&s, "a/k", ByteRange::Full).unwrap(), b"old");
    assert_eq!(s.head("a/new").unwrap(), None);
    assert_eq!(files_on_disk(s.root()), 1);
}

#[test]
fn a_conditional_create_never_overwrites() {
    let (s, _dir) = store("fs-create");
    let mut buf = [0u8; 16];
    assert_eq!(
        s.create_if_absent("k", &mut SliceSource::new(b"first"), &mut buf),
        Ok(CreateOutcome::Created)
    );
    assert_eq!(
        s.create_if_absent("k", &mut SliceSource::new(b"second"), &mut buf),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_eq!(get_all(&s, "k", ByteRange::Full).unwrap(), b"first");
    // A refused create is one attempt and, as S3's 412, one error.
    assert_counts(s.meter(), &[(S3Op::Put, 2), (S3Op::Get, 1)]);
    assert_eq!(s.meter().errors(), 1);
    assert_eq!(s.meter().bytes_written(), 5);
    assert_eq!(files_on_disk(s.root()), 1);
}

#[test]
fn of_racing_creates_exactly_one_wins_and_its_body_is_stored() {
    for round in 0..20 {
        let (s, _dir) = store("fs-race");
        let s = Arc::new(s);
        let gate = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8u8)
            .map(|t| {
                let (s, gate) = (Arc::clone(&s), Arc::clone(&gate));
                thread::spawn(move || {
                    let body = vec![t; 4096];
                    gate.wait();
                    let out = s
                        .create_if_absent("race/k", &mut SliceSource::new(&body), &mut [0; 512])
                        .unwrap();
                    (t, out)
                })
            })
            .collect();
        let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners: Vec<u8> = outcomes
            .iter()
            .filter(|(_, o)| *o == CreateOutcome::Created)
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(winners.len(), 1, "round {round}: {outcomes:?}");
        assert_eq!(
            get_all(&*s, "race/k", ByteRange::Full).unwrap(),
            vec![winners[0]; 4096]
        );
        assert_eq!(files_on_disk(s.root()), 1, "round {round}");
    }
}

#[test]
fn no_reader_sees_a_key_part_written() {
    const LEN: usize = 1 << 20;
    let (s, _dir) = store("fs-torn");
    put(&s, "k", &vec![0u8; LEN]);
    let s = Arc::new(s);
    let writers: Vec<_> = (1..=4u8)
        .map(|t| {
            let s = Arc::clone(&s);
            thread::spawn(move || {
                let body = vec![t; LEN];
                for _ in 0..15 {
                    s.put("k", &mut SliceSource::new(&body), &mut [0; 8192])
                        .unwrap();
                }
            })
        })
        .collect();
    let mut reads = 0;
    while writers.iter().any(|w| !w.is_finished()) || reads < 20 {
        let got = get_all(&*s, "k", ByteRange::Full).unwrap();
        assert_eq!(got.len(), LEN);
        assert!(
            got.iter().all(|b| *b == got[0]),
            "a torn read after {reads}"
        );
        reads += 1;
    }
    for w in writers {
        w.join().unwrap();
    }
}

#[test]
fn staged_files_are_never_listed() {
    let (s, _dir) = store("fs-staged");
    let s = Arc::new(s);
    let writer = {
        let s = Arc::clone(&s);
        thread::spawn(move || {
            let body = pattern(256 << 10);
            for i in 0..30 {
                s.put(
                    &format!("p/{i:02}"),
                    &mut SliceSource::new(&body),
                    &mut [0; 4096],
                )
                .unwrap();
            }
        })
    };
    while !writer.is_finished() {
        for (key, _) in list_all(&*s, "p/", 1000) {
            assert!(
                !key.contains("pending-") && !key.contains("register-locks"),
                "{key}"
            );
        }
    }
    writer.join().unwrap();
    assert_eq!(list_all(&*s, "", 1000).len(), 30);
}

#[test]
fn a_key_that_is_also_a_prefix_is_refused() {
    let (s, _dir) = store("fs-collide");
    put(&s, "a", b"x");
    let r = s.put("a/b", &mut SliceSource::new(b"y"), &mut [0; 4]);
    assert!(matches!(r, Err(BlobError::InvalidArgument(_))), "{r:?}");
    put(&s, "c/d", b"y");
    let r = s.put("c", &mut SliceSource::new(b"z"), &mut [0; 4]);
    assert!(matches!(r, Err(BlobError::InvalidArgument(_))), "{r:?}");
    let r = s.create_if_absent("c", &mut SliceSource::new(b"z"), &mut [0; 4]);
    assert_eq!(r, Ok(CreateOutcome::AlreadyExists));
}

// docs/specs/blob-batch-delete/SPEC.md AC-5
#[test]
fn a_batch_delete_on_disk_is_one_delete_a_key_after_checking_them_all() {
    let (s, _dir) = crate::store("batch");
    for k in ["d/a", "d/b"] {
        crate::put(&s, k, b"x");
    }
    assert!(matches!(
        s.delete_many(&["d/a", ""]),
        Err(BlobError::InvalidArgument(_))
    ));
    assert_eq!(
        s.head("d/a").unwrap().map(|m| m.len),
        Some(1),
        "nothing deleted"
    );
    s.delete_many(&["d/a", "d/b", "d/missing"]).unwrap();
    assert_eq!(s.head("d/a").unwrap(), None);
    assert_eq!(s.head("d/b").unwrap(), None);
    assert_eq!(s.meter().count(S3Op::Delete), 3);
}

/// An object's age is one HEAD: now less the file's modification time, both
/// read on the machine that holds the files, so one clock answers.
#[test]
fn an_objects_age_is_one_head_and_a_missing_key_has_none() {
    let (s, _dir) = store("fs-age");
    put(&s, "a", b"x");
    let age = s.age_millis("a").unwrap().expect("exists");
    assert!(age < 60_000, "a file just written is young: {age}");
    assert_eq!(s.age_millis("missing"), Ok(None));
    assert_counts(s.meter(), &[(S3Op::Put, 1), (S3Op::Head, 2)]);
}
