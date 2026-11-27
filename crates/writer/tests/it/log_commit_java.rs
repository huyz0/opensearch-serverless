//! docs/specs/open-from-logged-commit: a commit record laid out as the Java
//! shell lays it out, around a manifest the Java shell wrote, is one this
//! crate reads, orders and chooses from, and writes back byte for byte.

use std::path::PathBuf;

use oss_writer::log_commit::CommitRecord;
use oss_writer::takeover::logged::select;

/// The JSON of a manifest the Java shell wrote (fixtures/compat-blobs), term 1
/// and stage sequence 1.
#[allow(clippy::disallowed_methods)] // one small fixture, read whole
fn java_manifest() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/compat-blobs/segments/compat-term#compat-term-uuid#0/manifest");
    let bytes = std::fs::read(path).expect("fixture");
    let start = bytes.iter().position(|b| *b == b'{').expect("json");
    String::from_utf8(bytes[start..].to_vec()).expect("utf8")
}

/// What `LoggedCommits.java` builds: the oldest pack's time, the covered
/// epoch and ordinal (or `-1 -1`), a newline, the manifest.
fn java_record(packed_at: u64, covered: Option<(u64, u64)>, manifest: &str) -> String {
    let covered = covered.map_or_else(|| "-1 -1".to_owned(), |(e, o)| format!("{e} {o}"));
    format!("{packed_at} {covered}\n{manifest}")
}

fn register(term: u64, stage_seq: i64) -> Vec<u8> {
    format!("{{\"term\":{term},\"files\":{{}},\"stage_seq\":{stage_seq}}}").into_bytes()
}

fn at(text: &str) -> CommitRecord {
    CommitRecord::parse(text).expect("a record")
}

#[test]
fn a_record_the_java_shell_lays_out_parses_and_encodes_back_the_same() {
    let manifest = java_manifest();
    for covered in [None, Some((3, 17))] {
        let text = java_record(1_700_000_000_000, covered, &manifest);
        let record = at(&text);
        assert_eq!(record.oldest_pack_millis, 1_700_000_000_000);
        assert_eq!(record.covered.map(|p| (p.epoch, p.ordinal)), covered);
        assert_eq!(record.manifest, manifest);
        assert_eq!(record.encode(), text);
    }
}

#[test]
fn a_successor_chooses_a_java_written_commit_newer_than_the_register() {
    let manifest = java_manifest();
    let now = 1_700_000_100_000;
    let record = at(&java_record(now - 5_000, None, &manifest));
    let held = register(0, 0);
    let chosen = select(Some((&held, 9)), 2, &[record], now).expect("chosen");
    assert_eq!(chosen.manifest, manifest.as_bytes());
    assert_eq!(
        (chosen.term, chosen.stage_seq, chosen.generation),
        (1, 1, 9)
    );
}

#[test]
fn a_java_written_commit_the_register_already_covers_is_not_chosen() {
    let manifest = java_manifest();
    let now = 1_700_000_100_000;
    let record = at(&java_record(now - 5_000, None, &manifest));
    let held = register(1, 1);
    assert!(select(Some((&held, 9)), 2, std::slice::from_ref(&record), now).is_none());
    let held = register(0, 0);
    assert!(
        select(Some((&held, 9)), 1, &[record], now).is_none(),
        "from the term being won"
    );
}
