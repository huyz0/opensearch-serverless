//! The transport wire (docs/specs/transport/SPEC.md), checked against bytes
//! OpenSearch's own classes wrote (`fixtures/transport/`, recorded by
//! `WireRecordings.java`).

#![allow(clippy::disallowed_methods)]
// ARITH: indexes into fixture rows and small test counts; an overflow
// panics the test in a debug build, which fails it.
#![allow(clippy::arithmetic_side_effects)]

mod async_search;
mod exceptions;
mod fake_peer;
mod forwarding;
mod frames;
mod loopback;
mod mac_rotation;
mod macs;
mod nodes;
mod push;
mod raw;
mod read;
mod reader;
mod stream;
mod support;
mod tail;
mod tasks;
mod warm;

use std::net::IpAddr;
use std::path::PathBuf;

use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::version::WireVersion;

/// The tab-separated rows of fixture `name`.
pub(crate) fn rows(name: &str) -> Vec<Vec<String>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/transport")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect()
}

/// Bytes from hex, `-` for none.
pub(crate) fn bytes(text: &str) -> Vec<u8> {
    if text == "-" {
        return Vec::new();
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// A string from its UTF-8 in hex, `-` for empty.
pub(crate) fn unhex(text: &str) -> String {
    String::from_utf8(bytes(text)).unwrap()
}

/// Lower-case hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The host string Java's `InetSocketAddress` gives an address made from a
/// literal IP: the IP's text, IPv6 written out in full.
fn java_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => v6
            .segments()
            .iter()
            .map(|s| format!("{s:x}"))
            .collect::<Vec<_>>()
            .join(":"),
    }
}

fn address(ip: &str, port: i32) -> TransportAddress {
    let ip: IpAddr = ip.parse().unwrap();
    TransportAddress {
        ip,
        host: java_host(ip),
        port,
    }
}

/// The node an 11-column description in `nodes.txt` or `frames.txt` names.
pub(crate) fn node(fields: &[String]) -> DiscoveryNode {
    let port: i32 = fields[6].parse().unwrap();
    let attributes = if fields[8] == "-" {
        Vec::new()
    } else {
        fields[8]
            .split(';')
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap();
                (k.to_owned(), v.to_owned())
            })
            .collect()
    };
    let roles = if fields[9] == "-" {
        Vec::new()
    } else {
        // Abbreviations never hold a comma; a role's fields are colon-separated.
        fields[9]
            .split(',')
            .map(|r| {
                let parts: Vec<&str> = r.split(':').collect();
                NodeRole::new(parts[0], parts[1], parts[2] == "true")
            })
            .collect()
    };
    DiscoveryNode {
        name: unhex(&fields[0]),
        id: unhex(&fields[1]),
        ephemeral_id: unhex(&fields[2]),
        host_name: fields[3].clone(),
        host_address: fields[4].clone(),
        address: address(&fields[5], port),
        stream_address: (fields[7] != "-").then(|| address(&fields[7], port + 1)),
        attributes,
        roles,
        version: WireVersion::from_wire_id(fields[10].parse().unwrap()),
    }
}
