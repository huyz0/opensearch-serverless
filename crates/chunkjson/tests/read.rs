//! The `Read` adapter gives the slice API's events, for every buffer size.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use std::io::{self, Read};

use chunkjson::{Buffer, Config, Event, Framing, JsonReader, ReadError};
use common::{Builder, parse_chunked};
use proptest::prelude::*;
use serde_json::Value;

/// Returns at most `step` bytes per read, and an `Interrupted` before every other read.
struct Trickle<'a> {
    data: &'a [u8],
    step: usize,
    interrupt: bool,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let n = self.step.min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

fn read_all<B: Buffer>(
    data: &[u8],
    step: usize,
    buf: B,
    framing: Framing,
) -> Result<Builder, ReadError> {
    let reader = Trickle {
        data,
        step,
        interrupt: false,
    };
    let mut r = JsonReader::new(reader, buf, Config::with_framing(framing));
    let mut b = Builder::default();
    loop {
        let event = r.next_event()?;
        assert_ne!(event, Event::NeedMore);
        if !b.apply(event) {
            assert_eq!(r.tokenizer().offset(), data.len() as u64);
            return Ok(b);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn reader_matches_the_slice_api(v in any::<Vec<(String, i32, bool)>>(), step in 1usize..64, cap in 1usize..64) {
        let docs: Vec<Value> = v.iter().map(|(s, n, b)| serde_json::json!({"s": s, "n": n, "b": b})).collect();
        let mut body = Vec::new();
        for d in &docs {
            body.extend(serde_json::to_vec(d).unwrap());
            body.push(b'\n');
        }
        let from_reader = read_all(&body, step, vec![0u8; cap], Framing::Ndjson).unwrap();
        let from_slices = parse_chunked(&body, &[cap], Config::with_framing(Framing::Ndjson)).unwrap();
        prop_assert_eq!(&from_reader.docs, &docs);
        prop_assert_eq!(from_reader.docs, from_slices.docs);
        prop_assert_eq!(from_reader.ranges, from_slices.ranges);
    }
}

#[test]
fn every_buffer_type_reads() {
    let body = br#"{"a":[1,"two",null,false]}"#;
    let want = vec![serde_json::json!({"a": [1, "two", null, false]})];
    assert_eq!(
        read_all(body, 3, [0u8; 5], Framing::Single).unwrap().docs,
        want
    );
    assert_eq!(
        read_all(body, 3, vec![0u8; 7].into_boxed_slice(), Framing::Single)
            .unwrap()
            .docs,
        want
    );
    let mut backing = [0u8; 2];
    assert_eq!(
        read_all(body, 3, &mut backing[..], Framing::Single)
            .unwrap()
            .docs,
        want
    );
}

#[test]
fn errors_pass_through() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("disk on fire"))
        }
    }
    let mut r = JsonReader::new(Broken, vec![0u8; 8], Config::default());
    let err = r.next_event().unwrap_err();
    assert!(matches!(&err, ReadError::Io(e) if e.to_string() == "disk on fire"));
    assert_eq!(err.to_string(), "read failed: disk on fire");
    assert!(std::error::Error::source(&err).is_some());

    let err = read_all(b"[1,]", 2, vec![0u8; 3], Framing::Single).unwrap_err();
    let ReadError::Json(json) = &err else {
        panic!("json error expected")
    };
    assert_eq!(json.offset(), 3);
    assert_eq!(
        err.to_string(),
        "invalid JSON: unexpected byte 0x5d at offset 3"
    );
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn a_zero_length_buffer_reads_as_empty_input() {
    let err = read_all(b"[]", 1, Vec::new(), Framing::Single).unwrap_err();
    assert!(matches!(err, ReadError::Json(e) if e.kind() == chunkjson::ErrorKind::UnexpectedEof));
}
