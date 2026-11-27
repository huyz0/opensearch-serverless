//! docs/specs/stock-snapshot-export AC-7: a delete of a snapshot in a stock
//! repository is refused 501 and removes nothing from the repository.

#![allow(clippy::disallowed_methods)]

use crate::stock_export::{TAKE, WAIT, rig, send};

#[test]
fn a_delete_of_a_stock_snapshot_is_501_and_the_repository_is_untouched() {
    let r = rig(true, "delete");
    let taken = send(&r, "PUT", &format!("/_snapshot/stock/s1{WAIT}"), Some(TAKE));
    assert_eq!(taken.status, 200, "{}", taken.text());
    let before = r.written();
    let reply = send(&r, "DELETE", "/_snapshot/stock/s1", None);
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert!(reply.text().contains("stock"), "{}", reply.text());
    assert_eq!(r.written(), before, "nothing was removed or written");
    // A name the repository does not hold is the same refusal, not a 404 that
    // says the snapshot may be missing from a repository this node cannot read.
    let other = send(&r, "DELETE", "/_snapshot/stock/never", None);
    assert_eq!(other.status, 501, "{}", other.text());
}
