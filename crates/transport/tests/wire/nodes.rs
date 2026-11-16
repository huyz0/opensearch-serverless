//! AC-3: `DiscoveryNode` as the transport handshake writes it.

use oss_transport::WireError;
use oss_transport::node::DiscoveryNode;
use oss_transport::stream::{Input, Output};

use crate::{bytes, hex, node, rows};

#[test]
fn every_recorded_node_is_the_javas_both_ways() {
    let rows = rows("nodes.txt");
    assert_eq!(rows.len(), 10);
    for row in &rows {
        let with_attributes = match row[0].as_str() {
            "with_attributes" => true,
            "plain" => false,
            other => panic!("unknown row kind {other}"),
        };
        let mut expected = node(&row[1..12]);
        let mut buf = Vec::new();
        expected
            .write(&mut Output::new(&mut buf, 1 << 16), with_attributes)
            .unwrap();
        assert_eq!(hex(&buf), row[12], "{}", row[1]);
        let encoded = bytes(&row[12]);
        let mut input = Input::new(&encoded);
        let read = DiscoveryNode::read(&mut input).unwrap();
        input.finish("a node").unwrap();
        if !with_attributes {
            expected.attributes.clear();
        }
        assert_eq!(read, expected);
        for cut in 0..encoded.len() {
            assert!(DiscoveryNode::read(&mut Input::new(&encoded[..cut])).is_err());
        }
    }
}

#[test]
fn roles_read_sorted_once_each_and_addresses_are_four_or_sixteen_bytes() {
    let rows = rows("nodes.txt");
    let mut node = node(&rows[0][1..12]);
    let data = node.roles[0].clone();
    // Written out of order and repeated, read sorted and once.
    node.roles = vec![node.roles[1].clone(), data.clone(), data];
    let mut buf = Vec::new();
    node.write(&mut Output::new(&mut buf, 1 << 16), true)
        .unwrap();
    let read = DiscoveryNode::read(&mut Input::new(&buf)).unwrap();
    let names: Vec<&str> = read.roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["data", "ingest"]);

    // The first address length byte follows five strings.
    let encoded = bytes(&rows[0][12]);
    let mut input = Input::new(&encoded);
    for _ in 0..5 {
        input.string("s").unwrap();
    }
    let at = input.position();
    assert_eq!(encoded[at], 4);
    let mut broken = encoded.clone();
    broken[at] = 5;
    assert!(matches!(
        DiscoveryNode::read(&mut Input::new(&broken)),
        Err(WireError::Malformed { .. })
    ));
}

#[test]
fn the_shells_roles_are_the_built_in_ones() {
    let rows = rows("nodes.txt");
    let recorded = node(&rows[0][1..12]);
    assert_eq!(
        recorded.roles,
        vec![
            oss_transport::node::NodeRole::data(),
            oss_transport::node::NodeRole::ingest()
        ]
    );
}
