//! Node leases, members indexes and lease digests read and written as the
//! Java's `NodeLease`, `BlobLeaseMembership` and `LeaseDigest` do
//! (docs/specs/node-leases/SPEC.md, AC-1 and AC-2).

// ARITH: offsets and line numbers here are fixture sizes; an overflow
// panics the test in a debug build, which fails it.
#![allow(clippy::disallowed_methods, clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use oss_format::{
    FormatError, MAX_LEASE_BYTES, NodeLease, NodeLogEpoch, decode_digest, decode_members,
    encode_digest, encode_members, java_order, lease_digest_key, lease_key, members_index_key,
};

fn fixture(name: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/node-leases")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2).unwrap(), 16).unwrap())
        .collect()
}

fn written(lease: &NodeLease) -> String {
    let mut buf = vec![0u8; MAX_LEASE_BYTES];
    let n = lease.write(&mut buf).unwrap();
    buf.truncate(n);
    String::from_utf8(buf).unwrap()
}

#[test]
fn every_case_reads_and_writes_as_the_javas() {
    let cases = fixture("cases.txt");
    let parsed = fixture("parsed.txt");
    assert_eq!(parsed.len(), cases.len());
    assert!(cases.len() >= 24);
    let mut refused = 0;
    for (case, java) in cases.iter().zip(&parsed) {
        let rust = NodeLease::parse(case.as_bytes());
        if java == "refused" {
            refused += 1;
            assert!(
                matches!(rust, Err(FormatError::Corrupt { .. })),
                "{case}: {rust:?}"
            );
            continue;
        }
        let rust = rust.unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(written(&rust), *java, "{case}");
        let again = NodeLease::parse(java.as_bytes()).unwrap();
        assert_eq!(again, rust, "{case}");
    }
    assert!(refused >= 9, "the cases hold the Java's refusals");
}

#[test]
fn what_the_java_coerces_or_refuses_is_refused() {
    let strict = fixture("strict.txt");
    assert_eq!(strict.len(), fixture("strict-parsed.txt").len());
    for case in &strict {
        assert!(
            matches!(
                NodeLease::parse(case.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{case}"
        );
    }
}

#[test]
fn a_parsed_lease_says_what_the_javas_accessors_say() {
    let lease = NodeLease::parse(fixture("parsed.txt")[1].as_bytes()).unwrap();
    assert_eq!(lease.roles(), ["data", "ingest", "search"]);
    assert_eq!(lease.zone(), Some("us-east-1a"));
    assert_eq!(lease.display_name(), "node-two");
    assert_eq!(
        (lease.held, lease.cap, lease.in_use, lease.refused),
        (4, 100, 2, 0)
    );
    assert_eq!(
        lease.node_log,
        [
            NodeLogEpoch {
                epoch: 3,
                trim: 120,
                landed: 118
            },
            NodeLogEpoch {
                epoch: 4,
                trim: 0,
                landed: 0
            }
        ]
    );
    assert!(!lease.is_expired_at(1_700_000_029_999));
    assert!(lease.is_expired_at(1_700_000_030_000));
    let revoked = NodeLease::parse(fixture("parsed.txt")[2].as_bytes()).unwrap();
    assert!(revoked.revoked && revoked.is_expired_at(i64::MIN));
    let blank = NodeLease::parse(fixture("parsed.txt")[6].as_bytes()).unwrap();
    assert_eq!(blank.name.as_deref(), Some(""));
    assert_eq!(blank.display_name(), "n7", "a blank name shows the id");
    assert_eq!(
        NodeLease::new("n", "e", "a", 0).display_name(),
        "n",
        "no name shows the id"
    );
}

#[test]
fn a_built_lease_writes_as_the_java_would() {
    let mut lease = NodeLease::new("n2", "e2", "10.0.0.2:9300", 1_700_000_030_000);
    lease.set_roles(["search", "data", "ingest", "data"].map(str::to_owned));
    lease.name = Some("node-two".to_owned());
    lease.version = Some("3.3.0".to_owned());
    (lease.held, lease.cap, lease.in_use, lease.refused) = (4, 100, 2, 0);
    lease.set_zone(Some("us-east-1a".to_owned()));
    lease.node_log = vec![
        NodeLogEpoch {
            epoch: 3,
            trim: 120,
            landed: 118,
        },
        NodeLogEpoch {
            epoch: 4,
            trim: 0,
            landed: 0,
        },
    ];
    assert_eq!(written(&lease), fixture("parsed.txt")[1]);
    lease.set_zone(Some(" \t".to_owned()));
    assert_eq!(lease.zone(), None, "a blank zone is unknown");
}

#[test]
fn a_lease_too_long_is_refused_both_ways() {
    let mut lease = NodeLease::new("n", "e", "a", 0);
    lease.address = "x".repeat(MAX_LEASE_BYTES);
    let mut buf = vec![0u8; 2 * MAX_LEASE_BYTES];
    assert!(matches!(
        lease.write(&mut buf),
        Err(FormatError::TooLarge { actual, .. }) if actual > MAX_LEASE_BYTES as u64
    ));
    let mut short = [0u8; 10];
    assert!(matches!(
        NodeLease::new("n", "e", "a", 0).write(&mut short),
        Err(FormatError::TooLarge { actual: 81, .. })
    ));
    let long = format!(
        "{{\"node_id\":\"n\",\"ephemeral_id\":\"e\",\"address\":\"{}\"}}",
        "x".repeat(MAX_LEASE_BYTES)
    );
    assert!(matches!(
        NodeLease::parse(long.as_bytes()),
        Err(FormatError::TooLarge { .. })
    ));
}

/// The leases of each recorded fleet, as the recorder chose them.
fn fleets() -> Vec<Vec<NodeLease>> {
    let parsed = fixture("parsed.txt");
    [&[][..], &[1], &[1, 2, 3], &[2, 8, 12, 14, 24]]
        .iter()
        .map(|lines| {
            lines
                .iter()
                .map(|&l: &usize| NodeLease::parse(parsed[l - 1].as_bytes()).unwrap())
                .collect()
        })
        .collect()
}

#[test]
fn digests_read_and_write_as_the_javas() {
    let digests = fixture("digests.txt");
    for (f, (java, leases)) in digests.iter().zip(fleets()).enumerate() {
        let java = unhex(java);
        let digest = decode_digest(&java).unwrap();
        let at = 1_700_000_000_000 + i64::try_from(f).unwrap();
        assert_eq!(digest.written_at_millis, at);
        assert_eq!(digest.leases, leases);
        let mut buf = vec![0u8; java.len()];
        assert_eq!(encode_digest(at, &leases, &mut buf).unwrap(), java.len());
        assert_eq!(buf, java);
        if !java.is_empty() {
            let mut short = vec![0u8; java.len() - 1];
            assert!(matches!(
                encode_digest(at, &leases, &mut short),
                Err(FormatError::TooLarge { actual, .. }) if actual == java.len() as u64
            ));
        }
    }
}

#[test]
fn a_truncated_or_corrupt_digest_is_refused_at_every_cut() {
    let java = unhex(&fixture("digests.txt")[3]);
    for cut in 0..java.len() {
        assert!(decode_digest(&java[..cut]).is_err(), "cut at {cut}");
    }
    let mut longer = java.clone();
    longer.push(0);
    assert!(matches!(
        decode_digest(&longer),
        Err(FormatError::Corrupt { .. })
    ));
    let mut version = java.clone();
    version[0] = 2;
    assert!(matches!(
        decode_digest(&version),
        Err(FormatError::Unsupported { version: 2, .. })
    ));
    let mut count = java.clone();
    count[9] = 0x80;
    assert!(matches!(
        decode_digest(&count),
        Err(FormatError::Corrupt { .. })
    ));
    let mut huge = java.clone();
    huge[9..13].copy_from_slice(&i32::MAX.to_be_bytes());
    assert!(matches!(
        decode_digest(&huge),
        Err(FormatError::Truncated { .. })
    ));
    let mut length = java.clone();
    length[13] = 0x80;
    assert!(matches!(
        decode_digest(&length),
        Err(FormatError::Corrupt { .. })
    ));
    let mut lease = java;
    lease[17] = b'[';
    assert!(matches!(
        decode_digest(&lease),
        Err(FormatError::Corrupt { .. })
    ));
}

#[test]
fn a_lease_too_long_for_a_digest_is_skipped_and_the_rest_read() {
    let leases = &fleets()[2];
    let mut buf = vec![0u8; 16 << 10];
    let n = encode_digest(7, leases, &mut buf).unwrap();
    let mut value = buf[..n].to_vec();
    // A fourth entry the Java could write: a well-formed lease past the
    // bound, its write-log epochs unbounded in the Java.
    let mut long = String::from("{\"node_id\":\"n9\",\"pad\":\"");
    long.push_str(&"x".repeat(MAX_LEASE_BYTES));
    long.push_str("\"}");
    value[9..13].copy_from_slice(&4i32.to_be_bytes());
    value.extend_from_slice(&i32::try_from(long.len()).unwrap().to_be_bytes());
    value.extend_from_slice(long.as_bytes());
    let digest = decode_digest(&value).unwrap();
    assert_eq!(&digest.leases, leases);
}

#[test]
fn a_lease_of_exactly_the_bound_is_read_alone_and_in_a_digest() {
    let head = "{\"node_id\":\"n9\",\"ephemeral_id\":\"e\",\"address\":\"a\",\"expires_at_millis\":5,\"pad\":\"";
    let pad = MAX_LEASE_BYTES - head.len() - 2;
    let exact = format!("{head}{}\"}}", "x".repeat(pad));
    assert_eq!(exact.len(), MAX_LEASE_BYTES);
    assert_eq!(NodeLease::parse(exact.as_bytes()).unwrap().node_id, "n9");
    let mut value = vec![1u8];
    value.extend_from_slice(&7i64.to_be_bytes());
    value.extend_from_slice(&1i32.to_be_bytes());
    value.extend_from_slice(&i32::try_from(exact.len()).unwrap().to_be_bytes());
    value.extend_from_slice(exact.as_bytes());
    assert_eq!(ids_of(&decode_digest(&value).unwrap().leases), ["n9"]);
}

fn ids_of(leases: &[NodeLease]) -> Vec<&str> {
    leases.iter().map(|l| l.node_id.as_str()).collect()
}

#[test]
fn every_repeated_field_is_refused_not_only_the_first() {
    let base = "\"node_id\":\"n\",\"ephemeral_id\":\"e\",\"address\":\"a\",\"expires_at_millis\":5";
    assert!(NodeLease::parse(format!("{{{base}}}").as_bytes()).is_ok());
    for repeat in [
        "\"address\":\"b\"",
        "\"expires_at_millis\":6",
        "\"revoked\":true,\"revoked\":false",
    ] {
        let json = format!("{{{base},{repeat}}}");
        assert!(
            matches!(
                NodeLease::parse(json.as_bytes()),
                Err(FormatError::Corrupt { .. })
            ),
            "{json}"
        );
    }
}

#[test]
fn members_indexes_read_and_write_as_the_javas() {
    for (java, leases) in fixture("indexes.txt").iter().zip(fleets()) {
        let java = unhex(java);
        let ids = decode_members(&java).unwrap();
        let mut expected: Vec<String> = leases.iter().map(|l| l.node_id.clone()).collect();
        expected.sort_by(|a, b| java_order(a, b));
        assert_eq!(ids, expected);
        let mut buf = vec![0u8; java.len()];
        assert_eq!(encode_members(&ids, &mut buf).unwrap(), java.len());
        assert_eq!(buf, java);
    }
}

#[test]
fn an_index_is_read_as_the_javas_decode_ids() {
    let ids = decode_members(b"\n b \r\n\na\nb\n\t\n").unwrap();
    assert_eq!(
        ids,
        ["a", "b"],
        "trimmed, blank lines skipped, sorted, distinct"
    );
    assert!(decode_members(b"").unwrap().is_empty());
    assert!(decode_members(b"a/b").is_err());
    assert!(decode_members(&[0xff]).is_err());
    let mut buf = [0u8; 8];
    let unsorted = ["b", "a"].map(str::to_owned);
    assert!(encode_members(&unsorted, &mut buf).is_err());
    let repeated = ["a", "a"].map(str::to_owned);
    assert!(encode_members(&repeated, &mut buf).is_err());
    let long = ["abcd", "efgh"].map(str::to_owned);
    assert!(matches!(
        encode_members(&long, &mut buf),
        Err(FormatError::TooLarge { actual: 9, .. })
    ));
    assert_eq!(encode_members(&[], &mut buf).unwrap(), 0);
}

#[test]
fn java_order_is_by_utf16_unit() {
    // U+FFFD sorts after U+1F600 by UTF-16 unit (0xFFFD > 0xD83D), before
    // it by byte.
    assert!(java_order("\u{1F600}", "\u{FFFD}").is_lt());
    assert!("\u{1F600}" > "\u{FFFD}");
}

#[test]
fn keys_are_the_javas_register_map() {
    assert_eq!(
        lease_key("base/", "tZ9iGQAAQACwLK7Z_____w").unwrap(),
        "base/cluster/members/lease-tZ9iGQAAQACwLK7Z_____w"
    );
    assert_eq!(members_index_key("").unwrap(), "cluster/members/members");
    assert_eq!(
        lease_digest_key("").unwrap(),
        "cluster/members/members-digest"
    );
    for bad in ["", "a/b", "a b", "a\nb", &"x".repeat(129)] {
        assert!(lease_key("", bad).is_err(), "{bad:?}");
    }
    assert!(lease_key("", &"x".repeat(128)).is_ok());
    assert!(lease_key("base", "n").is_err());
    assert!(members_index_key("base").is_err());
    assert!(lease_digest_key("base").is_err());
}
