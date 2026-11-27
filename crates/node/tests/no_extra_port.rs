//! docs/specs/http2-front AC-1: with no edge listener set, the node opens no
//! extra port. Read from the kernel's socket tables for this process, which
//! runs this one test, so every socket it holds belongs to the node.

#![cfg(all(target_os = "linux", feature = "reader", feature = "writer"))]
// The sockets are read from /proc.
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeSet;

use oss_node::boot::{NodeConfig, start};
use oss_testkit::TempDir;

/// The inodes of the sockets this process holds.
fn socket_inodes() -> BTreeSet<u64> {
    std::fs::read_dir("/proc/self/fd")
        .expect("fds")
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter_map(|l| {
            let s = l.to_string_lossy().into_owned();
            s.strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse::<u64>()
                .ok()
        })
        .collect()
}

/// The local ports of this process's sockets in `table` (`tcp`, `udp`, ...):
/// listening ones only for TCP, every bound one for UDP.
fn ports(table: &str, mine: &BTreeSet<u64>, tcp: bool) -> BTreeSet<u16> {
    let text = std::fs::read_to_string(format!("/proc/net/{table}")).unwrap_or_default();
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (local, state, inode) = (f.get(1)?, f.get(3)?, f.get(9)?);
            if !mine.contains(&inode.parse().ok()?) || (tcp && *state != "0A") {
                return None;
            }
            u16::from_str_radix(local.rsplit(':').next()?, 16).ok()
        })
        .collect()
}

#[test]
fn with_no_edge_listener_set_the_node_listens_on_its_http_and_transport_ports_only() {
    let dir = TempDir::new("no-extra-port");
    let root = dir.path().display().to_string();
    let lookup = |key: &str| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
        _ => None,
    };
    let node = start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts");
    let mine = socket_inodes();
    let mut tcp = ports("tcp", &mine, true);
    tcp.extend(ports("tcp6", &mine, true));
    let mut udp = ports("udp", &mine, false);
    udp.extend(ports("udp6", &mine, false));
    let expected: BTreeSet<u16> = [node.addr().port(), node.transport_addr().port()]
        .into_iter()
        .collect();
    assert_eq!(
        tcp, expected,
        "listening TCP ports: the HTTP and the transport"
    );
    assert!(udp.is_empty(), "no UDP socket is bound: {udp:?}");
}
