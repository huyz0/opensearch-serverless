//! The warm-copy messages: a joining reader asks a peer which pages it holds
//! hot, then asks for some of them. These are Rust-only actions, so the
//! checks are round trips, every shorter cut, and the bounds a decoder holds
//! before it sizes anything.

use oss_transport::actions::tail::ShardRef;
use oss_transport::actions::warm::{
    HOT_SET_ACTION, HotFile, HotPage, HotSetRequest, HotSetResponse, MAX_PAGES_PER_REQUEST,
    PAGES_ACTION, PageAsk, PageBytes, PagesRequest, PagesResponse,
};
use oss_transport::error::WireError;
use oss_transport::stream::{Input, Output, TaskId};

fn encode(write: impl FnOnce(&mut Output<'_>) -> Result<(), WireError>) -> Vec<u8> {
    let mut buf = Vec::new();
    write(&mut Output::new(&mut buf, 1 << 20)).unwrap();
    buf
}

fn read_whole<T>(encoded: &[u8], read: impl Fn(&mut Input<'_>) -> Result<T, WireError>) -> T {
    for cut in 0..encoded.len() {
        assert!(
            read(&mut Input::new(&encoded[..cut])).is_err(),
            "a cut at {cut} reads"
        );
    }
    let mut input = Input::new(encoded);
    let value = read(&mut input).unwrap();
    assert_eq!(input.remaining(), 0);
    value
}

fn shard() -> ShardRef {
    ShardRef {
        index_name: "logs".into(),
        index_uuid: "u1".into(),
        shard: 3,
    }
}

#[test]
fn the_actions_are_named_under_the_warm_family() {
    assert_eq!(HOT_SET_ACTION, "internal:serverless/warm/hot_set");
    assert_eq!(PAGES_ACTION, "internal:serverless/warm/pages");
}

#[test]
fn a_hot_set_request_round_trips() {
    let request = HotSetRequest {
        parent: TaskId::default(),
        joiner: "node-9".into(),
        max_shards: 1000,
        max_bytes: 1 << 33,
    };
    let bytes = encode(|out| request.write(out));
    assert_eq!(read_whole(&bytes, HotSetRequest::read), request);
}

#[test]
fn a_hot_set_response_round_trips_and_keeps_the_order_of_shards_files_and_pages() {
    let response = HotSetResponse {
        page_size: 65_536,
        shards: vec![
            (
                shard(),
                vec![
                    HotFile {
                        key: "b/i/0/t=1\0_0.cfs\0900\0p".into(),
                        pages: vec![
                            HotPage {
                                page: 0,
                                len: 65_536,
                            },
                            HotPage {
                                page: 7,
                                len: 1_000,
                            },
                        ],
                    },
                    HotFile {
                        key: "b/i/0/t=1\0_1.fdt\x00100\0p".into(),
                        pages: vec![HotPage { page: 0, len: 100 }],
                    },
                ],
            ),
            (ShardRef::default(), Vec::new()),
        ],
    };
    let bytes = encode(|out| response.write(out));
    assert_eq!(read_whole(&bytes, HotSetResponse::read), response);
}

#[test]
fn a_pages_request_round_trips_and_a_request_past_the_cap_is_refused_before_it_is_read() {
    let ask = |n: usize| PagesRequest {
        parent: TaskId::default(),
        asks: (0..n)
            .map(|i| PageAsk {
                key: format!("k{i}"),
                page: u64::try_from(i).unwrap(),
            })
            .collect(),
    };
    let at_cap = ask(MAX_PAGES_PER_REQUEST);
    let bytes = encode(|out| at_cap.write(out));
    assert_eq!(read_whole(&bytes, PagesRequest::read), at_cap);

    let over = ask(MAX_PAGES_PER_REQUEST + 1);
    let bytes = encode(|out| over.write(out));
    assert!(matches!(
        PagesRequest::read(&mut Input::new(&bytes)),
        Err(WireError::TooLarge { .. })
    ));
}

#[test]
fn a_pages_response_round_trips_with_each_page_as_it_was_held() {
    let response = PagesResponse {
        pages: vec![
            PageBytes {
                key: "k0".into(),
                page: 2,
                bytes: vec![1, 2, 3],
            },
            PageBytes {
                key: "k1".into(),
                page: 0,
                bytes: Vec::new(),
            },
        ],
    };
    let bytes = encode(|out| response.write(out));
    assert_eq!(read_whole(&bytes, PagesResponse::read), response);
}

#[test]
fn counts_the_bytes_cannot_hold_are_refused_without_sizing_for_them() {
    // A hot-set answer claiming a million shards in a handful of bytes.
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 64);
    out.vint(65_536).unwrap();
    out.count(1_000_000).unwrap();
    assert!(HotSetResponse::read(&mut Input::new(&buf)).is_err());
}

#[test]
fn a_negative_page_is_refused() {
    let mut buf = Vec::new();
    let mut out = Output::new(&mut buf, 64);
    TaskId::default().write(&mut out).unwrap();
    out.count(1).unwrap();
    out.string("k").unwrap();
    // -1 as a vlong: nine continuation bytes and a one.
    out.bytes(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01])
        .unwrap();
    assert!(PagesRequest::read(&mut Input::new(&buf)).is_err());
}

#[test]
fn a_pages_response_at_the_cap_reads_and_one_past_it_is_refused() {
    let served = |n: usize| PagesResponse {
        pages: (0..n)
            .map(|i| PageBytes {
                key: format!("k{i}"),
                page: 0,
                bytes: vec![0; 2],
            })
            .collect(),
    };
    let at_cap = served(MAX_PAGES_PER_REQUEST);
    let bytes = encode(|out| at_cap.write(out));
    assert_eq!(read_whole(&bytes, PagesResponse::read), at_cap);

    let bytes = encode(|out| served(MAX_PAGES_PER_REQUEST + 1).write(out));
    assert!(matches!(
        PagesResponse::read(&mut Input::new(&bytes)),
        Err(WireError::TooLarge { .. })
    ));
}
