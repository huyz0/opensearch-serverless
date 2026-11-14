//! PUTs, conditional creates and the capacity bound.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::{MIB, emulator, get_all, put};
use oss_blob::*;

// AC-5
#[test]
fn a_conditional_create_never_overwrites() {
    let e = emulator();
    put(&e, "log/1", b"first");
    let r = e.create_if_absent("log/1", &mut SliceSource::new(b"second"), &mut [0; 4]);
    assert_eq!(r, Ok(CreateOutcome::AlreadyExists));
    assert_eq!(get_all(&e, "log/1", ByteRange::Full).unwrap(), b"first");
    assert_eq!(e.stored_bytes(), 5);
}

/// A body whose first fill waits until every racer has started streaming,
/// so all of them are past any check made before the body is read. The wait
/// gives up after two seconds, so a racer that never arrives fails the test
/// instead of hanging it.
struct GatedSource {
    gate: Arc<AtomicUsize>,
    inner: SliceSource<'static>,
    opened: bool,
}

impl ChunkSource for GatedSource {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        if !self.opened {
            self.opened = true;
            self.gate.fetch_add(1, Ordering::SeqCst);
            let start = Instant::now();
            while self.gate.load(Ordering::SeqCst) < 8 && start.elapsed() < Duration::from_secs(2) {
                std::thread::yield_now();
            }
        }
        self.inner.fill(buf)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.inner.rewind()
    }
}

#[test]
fn of_racing_creates_exactly_one_wins() {
    const BODIES: [[u8; 512]; 8] = [
        [0; 512], [1; 512], [2; 512], [3; 512], [4; 512], [5; 512], [6; 512], [7; 512],
    ];
    for round in 0..20 {
        let e = Arc::new(Emulator::new(MIB, round));
        let gate = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = BODIES
            .iter()
            .map(|body| {
                let (e, gate) = (Arc::clone(&e), Arc::clone(&gate));
                std::thread::spawn(move || {
                    let mut src = GatedSource {
                        gate,
                        inner: SliceSource::new(body),
                        opened: false,
                    };
                    e.create_if_absent("fence", &mut src, &mut [0; 64]).unwrap()
                })
            })
            .collect();
        let won: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            won.iter().filter(|o| **o == CreateOutcome::Created).count(),
            1,
            "round {round}"
        );
        assert_eq!(e.meter().count(S3Op::Put), 8);
        assert_eq!(e.meter().errors(), 7);
        assert_eq!(e.stored_bytes(), 512);
    }
}

#[test]
fn a_create_of_a_taken_key_is_refused_before_capacity_is_checked() {
    let e = Emulator::new(10, 1);
    put(&e, "fence", &[0; 8]);
    let r = e.create_if_absent("fence", &mut SliceSource::new(&[1; 8]), &mut [0; 4]);
    assert_eq!(
        r,
        Ok(CreateOutcome::AlreadyExists),
        "a 412, not a capacity error"
    );
}

#[test]
fn a_lost_response_still_commits_and_a_retried_create_finds_its_own_object() {
    let e = emulator();
    e.lose_response_next(S3Op::Put, 1);
    let mut src = SliceSource::new(b"mine");
    assert_eq!(
        e.create_if_absent("log/7", &mut src, &mut [0; 4]),
        Err(BlobError::Unavailable)
    );
    assert_eq!(
        get_all(&e, "log/7", ByteRange::Full).unwrap(),
        b"mine",
        "the write took effect"
    );
    src.rewind().unwrap();
    assert_eq!(
        e.create_if_absent("log/7", &mut src, &mut [0; 4]),
        Ok(CreateOutcome::AlreadyExists)
    );
    assert_eq!(e.meter().count(S3Op::Put), 2);
    assert_eq!(e.meter().errors(), 2);
}

/// A source that declares one length and yields another, or fails.
struct BadSource {
    declared: u64,
    yields: usize,
    fail_after: Option<usize>,
    sent: usize,
}

impl ChunkSource for BadSource {
    fn len(&self) -> u64 {
        self.declared
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        if self.fail_after.is_some_and(|n| self.sent >= n) {
            return Err(BlobError::Stream("disk read failed"));
        }
        let n = buf.len().min(self.yields.saturating_sub(self.sent));
        self.sent = self.sent.saturating_add(n);
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        Err(BlobError::Stream("not repeatable"))
    }
}

// AC-6
#[test]
fn a_put_with_a_bad_body_leaves_the_old_object() {
    let e = emulator();
    put(&e, "k", b"old");
    let cases = [
        (
            BadSource {
                declared: 100,
                yields: 50,
                fail_after: None,
                sent: 0,
            },
            "shorter",
        ),
        (
            BadSource {
                declared: 100,
                yields: 150,
                fail_after: None,
                sent: 0,
            },
            "longer",
        ),
        (
            BadSource {
                declared: 100,
                yields: 100,
                fail_after: Some(40),
                sent: 0,
            },
            "fails",
        ),
    ];
    for (mut src, why) in cases {
        assert!(e.put("k", &mut src, &mut [0; 32]).is_err(), "{why}");
        assert_eq!(get_all(&e, "k", ByteRange::Full).unwrap(), b"old", "{why}");
    }
    let mut src = BadSource {
        declared: 10,
        yields: 5,
        fail_after: None,
        sent: 0,
    };
    assert!(e.create_if_absent("new", &mut src, &mut [0; 32]).is_err());
    assert!(src.rewind().is_err());
    assert_eq!(e.head("new"), Ok(None));
    assert_eq!((e.stored_bytes(), e.object_count()), (3, 1));
    assert_eq!(e.meter().errors(), 4);
    assert_eq!(e.meter().bytes_written(), 3);
}

// AC-8
#[test]
fn the_emulator_refuses_writes_past_its_capacity() {
    let e = Emulator::new(100, 1);
    assert_eq!(e.object_count(), 0);
    put(&e, "a", &[0; 60]);
    let r = e.put("b", &mut SliceSource::new(&[0; 41]), &mut [0; 16]);
    assert_eq!(r, Err(BlobError::CapacityExceeded));
    assert_eq!(e.head("b"), Ok(None));
    put(&e, "b", &[0; 40]);
    assert_eq!(e.stored_bytes(), 100);
    e.delete("a").unwrap();
    assert_eq!(e.stored_bytes(), 40);
    // A replace holds the old body until it commits, so it needs room for both.
    assert_eq!(
        e.put("b", &mut SliceSource::new(&[0; 61]), &mut [0; 16]),
        Err(BlobError::CapacityExceeded)
    );
    put(&e, "b", &[0; 60]);
    assert_eq!(e.stored_bytes(), 60);
    put(&e, "c", &[0; 40]);
    assert_eq!(
        e.put("c", &mut SliceSource::new(&[0; 1]), &mut [0; 1]),
        Err(BlobError::CapacityExceeded)
    );
    assert_eq!(e.meter().count(S3Op::Put), 7);
    assert_eq!(e.meter().errors(), 3);
}

#[test]
fn a_failed_put_gives_its_reservation_back() {
    let e = Emulator::new(100, 1);
    e.fail_next(S3Op::Put, 1);
    assert_eq!(
        e.put("a", &mut SliceSource::new(&[0; 100]), &mut [0; 16]),
        Err(BlobError::Unavailable)
    );
    let mut short = SliceSource::new(&[0; 10]);
    let mut lying = Declared {
        inner: &mut short,
        len: 100,
    };
    assert!(e.put("a", &mut lying, &mut [0; 16]).is_err());
    put(&e, "a", &[0; 50]);
    put(&e, "b", &[0; 50]);
    assert_eq!((e.stored_bytes(), e.object_count()), (100, 2));
}

/// Declares a length its inner source does not have.
struct Declared<'a> {
    inner: &'a mut SliceSource<'static>,
    len: u64,
}

impl ChunkSource for Declared<'_> {
    fn len(&self) -> u64 {
        self.len
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        self.inner.fill(buf)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.inner.rewind()
    }
}
