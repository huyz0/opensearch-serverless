//! docs/specs/restore-default-flags: the flags at the value this node
//! applies cost nothing over REST.

use crate::snapshot_restore::{RESTORE, requests, send, taken};

#[test]
fn flags_at_their_defaults_cost_nothing() {
    let a = taken(true);
    let b = taken(true);
    let (a0, b0) = (requests(&a), requests(&b));
    let plain = r#"{"rename_pattern":"logs","rename_replacement":"copy"}"#;
    let flags = r#"{"rename_pattern":"logs","rename_replacement":"copy","partial":false,"include_global_state":false,"include_aliases":false,"ignore_unavailable":false}"#;
    assert_eq!(send(&a, "POST", RESTORE, Some(plain)).status, 200);
    let reply = send(&b, "POST", RESTORE, Some(flags));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(requests(&a) - a0, requests(&b) - b0);
    let refused =
        r#"{"rename_pattern":"logs","rename_replacement":"two","include_global_state":true}"#;
    assert_eq!(send(&b, "POST", RESTORE, Some(refused)).status, 501);
}
