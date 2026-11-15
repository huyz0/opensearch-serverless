//! Tests; compiled only under `#[cfg(test)]` from the parent module.

#[cfg(test)]
use std::sync::Arc;
use std::time::Duration;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};

use super::*;
use crate::inbound::Inbound;

fn budget(bytes: usize) -> MemoryBudget {
    let c = BudgetConfig::new(bytes)
        .with_pool(Pool::RequestBodies, bytes)
        .expect("fits");
    MemoryBudget::new(&c)
}

fn collect(body: &mut Body<'_>) -> Result<Vec<u8>, RestError> {
    let mut all = Vec::new();
    while let Some(chunk) = body.next_chunk()? {
        assert!(chunk.len() <= 8, "a chunk is at most one buffer");
        all.extend_from_slice(chunk);
    }
    Ok(all)
}

fn run(raw: &[u8], framing: Framing, max: u64) -> Result<Vec<u8>, RestError> {
    let b = budget(1 << 16);
    let bufs = Arc::new(BodyBuffers::new(&b, 8, 1).expect("fits"));
    let mut inb = Inbound::new(raw, [0u8; 64]);
    let buffer = bufs.checkout(Duration::ZERO).ok();
    let mut body = Body::new(&mut inb, framing, buffer, max, 64, &b);
    collect(&mut body)
}

#[test]
fn a_length_body_streams_in_buffer_sized_chunks() {
    let got = run(b"hello world, streamed", Framing::Length(21), 100).expect("reads");
    assert_eq!(got, b"hello world, streamed");
}

#[test]
fn bytes_past_the_length_are_left_for_the_next_request() {
    let b = budget(1 << 16);
    let bufs = Arc::new(BodyBuffers::new(&b, 8, 1).expect("fits"));
    let mut inb = Inbound::new(&b"abcNEXT"[..], [0u8; 64]);
    inb.fill().expect("fills");
    let buffer = bufs.checkout(Duration::ZERO).ok();
    let mut body = Body::new(&mut inb, Framing::Length(3), buffer, 100, 64, &b);
    assert_eq!(body.declared_length(), Some(3));
    assert!(format!("{body:?}").starts_with("Body {"));
    assert_eq!(collect(&mut body).expect("reads"), b"abc");
    assert!(body.is_done());
    assert_eq!(body.bytes_read(), 3);
    drop(body);
    assert_eq!(inb.buffered(), b"NEXT");
}

#[test]
fn a_chunk_is_never_larger_than_the_buffer_says() {
    let b = budget(1 << 16);
    let bufs = Arc::new(BodyBuffers::new(&b, 8, 1).expect("fits"));
    let mut inb = Inbound::new(&b"0123456789abcdef"[..], [0u8; 64]);
    let buffer = bufs.checkout(Duration::ZERO).ok();
    let mut body = Body::new(&mut inb, Framing::Length(16), buffer, 100, 64, &b);
    assert_eq!(body.max_chunk(), 8);
    while let Some(chunk) = body.next_chunk().expect("reads") {
        assert!(chunk.len() <= 8);
    }
    let mut none = Inbound::new(&b""[..], [0u8; 64]);
    assert_eq!(
        Body::new(&mut none, Framing::Empty, None, 0, 64, &b).max_chunk(),
        0
    );
}

#[test]
fn a_short_body_is_an_error_that_closes() {
    let e = run(b"abc", Framing::Length(5), 100).expect_err("short");
    assert!(e.closes_connection());
}

#[test]
fn a_chunked_body_decodes_with_extensions_and_trailers() {
    let raw = b"5;ext=1\r\nhello\r\n7\r\n, world\r\nA\n0123456789\n0\r\nX-T: 1\r\n\r\n";
    let got = run(raw, Framing::Chunked(Chunk::Size), 100).expect("reads");
    assert_eq!(got, b"hello, world0123456789");
}

#[test]
fn the_chunked_limit_holds_at_max_and_refuses_one_more() {
    let raw = b"5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
    assert_eq!(
        run(raw, Framing::Chunked(Chunk::Size), 10).expect("exactly the limit"),
        b"helloworld"
    );
    let e = run(raw, Framing::Chunked(Chunk::Size), 9).expect_err("one past");
    assert_eq!(e.status(), 413);
    assert!(e.closes_connection());
}

#[test]
fn malformed_chunks_are_refused() {
    for raw in [
        &b"zz\r\nhello\r\n0\r\n\r\n"[..],
        b"\r\nhello",
        b"5\r\nhelloXX0\r\n\r\n",
        b"11111111111111111\r\n",
        b"5\r\nhel",
        b"5\r\nhello\r\n0\r\nX: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n\r\n",
    ] {
        let e = run(raw, Framing::Chunked(Chunk::Size), 1 << 20).expect_err("malformed");
        assert!(e.closes_connection(), "{raw:?}");
    }
    let long = [b'1'; MAX_CHUNK_LINE + 8];
    let mut inb = Inbound::new(&long[..], [0u8; 2048]);
    let b = budget(1 << 16);
    let mut body = Body::new(&mut inb, Framing::Chunked(Chunk::Size), None, 100, 64, &b);
    assert!(body.next_chunk().is_err());
}

#[test]
fn an_empty_body_has_no_chunks() {
    assert_eq!(run(b"", Framing::Empty, 0).expect("empty"), b"");
    assert_eq!(run(b"", Framing::Length(0), 0).expect("empty"), b"");
    let b = budget(64);
    let mut src = Inbound::new(&b""[..], [0u8; 4]);
    let body = Body::new(&mut src, Framing::Chunked(Chunk::Size), None, 0, 64, &b);
    assert_eq!(body.declared_length(), None);
    let mut src = Inbound::new(&b"x"[..], [0u8; 4]);
    let mut body = Body::new(&mut src, Framing::Length(1), None, 1, 64, &b);
    assert!(body.next_chunk().is_err(), "a body needs a buffer");
}

#[test]
fn reservations_grow_to_the_pool_and_refuse_one_more_byte() {
    let b = budget(100);
    let mut src = Inbound::new(&b""[..], [0u8; 4]);
    let mut body = Body::new(&mut src, Framing::Empty, None, 0, 64, &b);
    body.reserve(60).expect("fits");
    body.reserve(40).expect("exactly the pool");
    assert_eq!(body.reserved(), 100);
    let e = body.reserve(1).expect_err("one past");
    assert_eq!(e.status(), 429);
    assert_eq!(
        e.as_exception().map(|x| x.kind()),
        Some("circuit_breaking_exception")
    );
    drop(body);
    assert_eq!(
        b.in_use(Pool::RequestBodies),
        0,
        "released with the request"
    );
}

#[test]
fn checkout_takes_count_buffers_and_refuses_one_more() {
    let b = budget(1 << 16);
    let bufs = Arc::new(BodyBuffers::new(&b, 1024, 2).expect("fits"));
    let one = bufs.checkout(Duration::ZERO).expect("first");
    let two = bufs.checkout(Duration::ZERO).expect("second");
    assert_eq!(bufs.available(), 0);
    let e = bufs
        .checkout(Duration::from_millis(20))
        .expect_err("one past, after the wait");
    assert_eq!(e.pool, Pool::RequestBodies);
    drop(one);
    assert_eq!(bufs.available(), 1);
    drop(two);
    assert!(
        BodyBuffers::new(&b, 1 << 15, 3).is_err(),
        "more than the pool holds"
    );
}

#[test]
fn a_waiting_checkout_gets_the_next_buffer_returned() {
    let b = budget(1 << 16);
    let bufs = Arc::new(BodyBuffers::new(&b, 1024, 1).expect("fits"));
    let held = bufs.checkout(Duration::ZERO).expect("first");
    let waiter = {
        let bufs = Arc::clone(&bufs);
        std::thread::spawn(move || bufs.checkout(Duration::from_secs(10)).map(|_| ()))
    };
    std::thread::sleep(Duration::from_millis(30));
    drop(held);
    assert!(
        waiter.join().expect("joins").is_ok(),
        "woken by the returned buffer"
    );
}
