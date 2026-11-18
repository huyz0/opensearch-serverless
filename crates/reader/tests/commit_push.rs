//! The mailbox a writer's pushed commits wait in, and the order that picks
//! the newer of two commits.

use oss_format::{ManifestEntry, encode_manifest};
use oss_reader::commit_push::{
    KEEP_MILLIS, PushedCommits, Refused, ShardKey, Stage, Taken, newer, newer_than, stage_of,
    staged_after,
};

fn commit(term: u64, writer: Option<&str>, stage: i64) -> Vec<u8> {
    encode_manifest(term, writer, &[], 0, stage).expect("encode")
}

/// A commit that names a `segments_N` file.
fn commit_at(term: u64, writer: Option<&str>, stage: i64, segments: &str) -> Vec<u8> {
    let entry = ManifestEntry {
        name: segments,
        location: "t=1/_1.seg#0:10".to_owned(),
        length: 10,
    };
    encode_manifest(term, writer, &[entry], 0, stage).expect("encode")
}

/// What a take of the commit pushed under uuid `u` gives.
fn taken(value: &[u8]) -> Taken {
    Taken {
        uuid: "u".to_owned(),
        value: value.to_vec(),
    }
}

fn key(shard: u32) -> ShardKey {
    ShardKey::new("logs", shard)
}

fn stage(term: u64, writer: Option<&str>, sequence: i64) -> Stage {
    Stage::new(term, writer.map(str::to_owned), sequence)
}

#[test]
fn a_push_is_kept_until_taken_and_taken_once() {
    let mut m = PushedCommits::new(8, 1 << 20);
    let value = commit(2, Some("w"), 4);
    assert!(m.is_empty());
    assert_eq!(m.push(&key(0), "u", &value, 100), Ok(true));
    assert!(!m.is_empty());
    assert_eq!(m.len(), 1);
    assert_eq!(m.held_bytes(), value.len());
    assert_eq!(m.take(&key(1), 100), None, "another shard's");
    assert_eq!(m.take(&key(0), 100), Some(taken(&value)));
    assert_eq!(m.take(&key(0), 100), None);
    assert_eq!((m.len(), m.held_bytes()), (0, 0));
    assert!(m.is_empty());
}

#[test]
fn a_push_older_than_the_keep_time_is_not_handed_out() {
    let mut m = PushedCommits::new(8, 1 << 20);
    let value = commit(2, Some("w"), 4);
    m.push(&key(0), "u", &value, 1_000).unwrap();
    assert_eq!(
        m.take(&key(0), 1_000 + KEEP_MILLIS),
        Some(taken(&value)),
        "exactly as old as the keep time still counts"
    );
    m.push(&key(0), "u", &value, 1_000).unwrap();
    assert_eq!(m.take(&key(0), 1_001 + KEEP_MILLIS), None);
    assert_eq!((m.len(), m.held_bytes()), (0, 0), "dropped, not kept");
    // A clock that went back is not an age.
    m.push(&key(0), "u", &value, 5_000).unwrap();
    assert_eq!(m.take(&key(0), 1_000), Some(taken(&value)));
}

#[test]
fn only_a_newer_push_replaces_the_one_held() {
    let mut m = PushedCommits::new(8, 1 << 20);
    let five = commit(2, Some("w"), 5);
    assert_eq!(m.push(&key(0), "u", &five, 0), Ok(true));
    assert_eq!(m.push(&key(0), "u", &commit(2, Some("w"), 3), 0), Ok(false));
    assert_eq!(
        m.push(&key(0), "u", &commit(2, Some("w"), 5), 0),
        Ok(false),
        "the same commit again is not newer"
    );
    let seven = commit(2, Some("w"), 7);
    assert_eq!(m.push(&key(0), "u", &seven, 0), Ok(true));
    assert_eq!(m.held_bytes(), seven.len(), "one commit a shard");
    // Another writer in the same term is not ordered with it: the one held
    // stays, and a lower term never replaces a higher one.
    assert_eq!(m.push(&key(0), "u", &commit(2, Some("x"), 9), 0), Ok(false));
    assert_eq!(
        m.push(&key(0), "u", &commit(1, Some("w"), 99), 0),
        Ok(false)
    );
    // A higher term replaces whatever the stages say.
    let term = commit(3, Some("x"), 0);
    assert_eq!(m.push(&key(0), "u", &term, 0), Ok(true));
    assert_eq!(m.take(&key(0), 0), Some(taken(&term)));
}

#[test]
fn a_higher_lucene_generation_is_newer_when_no_stage_orders_them() {
    let mut m = PushedCommits::new(8, 1 << 20);
    let a = commit_at(2, Some("w"), -1, "segments_5");
    assert_eq!(m.push(&key(0), "u", &a, 0), Ok(true));
    assert_eq!(
        m.push(&key(0), "u", &commit_at(2, Some("w"), -1, "segments_4"), 0),
        Ok(false)
    );
    // Generations are base 36: `a` is ten, past five.
    let ten = commit_at(2, Some("x"), -1, "segments_a");
    assert_eq!(m.push(&key(0), "u", &ten, 0), Ok(true));
    assert_eq!(m.take(&key(0), 0), Some(taken(&ten)));
}

#[test]
fn bytes_that_are_not_a_commit_are_refused_and_held_nowhere() {
    let mut m = PushedCommits::new(8, 1 << 20);
    assert_eq!(
        m.push(&key(0), "u", b"not a commit", 0),
        Err(Refused::NotACommit)
    );
    assert_eq!(m.push(&key(0), "u", b"", 0), Err(Refused::NotACommit));
    assert_eq!((m.len(), m.held_bytes()), (0, 0));
}

#[test]
fn past_the_shard_or_byte_bound_a_push_is_refused_and_the_held_stay() {
    let one = commit(2, Some("w"), 1);
    let mut m = PushedCommits::new(2, 1 << 20);
    m.push(&key(0), "u", &one, 0).unwrap();
    m.push(&key(1), "u", &one, 0).unwrap();
    assert_eq!(m.push(&key(2), "u", &one, 0), Err(Refused::Full));
    // A shard already held is not a new shard.
    assert_eq!(m.push(&key(1), "u", &commit(2, Some("w"), 2), 0), Ok(true));
    assert_eq!(m.len(), 2);

    let mut m = PushedCommits::new(8, one.len() * 2);
    m.push(&key(0), "u", &one, 0).unwrap();
    m.push(&key(1), "u", &one, 0).unwrap();
    assert_eq!(m.push(&key(2), "u", &one, 0), Err(Refused::Full));
    assert_eq!(m.held_bytes(), one.len() * 2);
    // Replacing counts the difference, not both.
    assert_eq!(m.push(&key(0), "u", &commit(2, Some("w"), 2), 0), Ok(true));
    // A taken push gives its bytes back.
    m.take(&key(0), 0).unwrap();
    assert_eq!(m.push(&key(2), "u", &one, 0), Ok(true));
}

#[test]
fn a_full_mailbox_makes_room_by_dropping_what_is_past_its_keep_time() {
    let one = commit(2, Some("w"), 1);
    let mut m = PushedCommits::new(1, 1 << 20);
    m.push(&key(0), "u", &one, 0).unwrap();
    assert_eq!(m.push(&key(1), "u", &one, KEEP_MILLIS), Err(Refused::Full));
    assert_eq!(m.push(&key(1), "u", &one, KEEP_MILLIS + 1), Ok(true));
    assert_eq!(m.len(), 1);
    assert_eq!(m.take(&key(0), KEEP_MILLIS + 1), None);
}

#[test]
fn a_commit_is_staged_after_another_only_by_one_writer_in_one_term() {
    let base = stage(2, Some("w"), 5);
    assert!(staged_after(&stage(2, Some("w"), 6), &base));
    assert!(!staged_after(&stage(2, Some("w"), 5), &base));
    assert!(!staged_after(&stage(2, Some("w"), 4), &base));
    assert!(!staged_after(&stage(3, Some("w"), 6), &base), "term");
    assert!(!staged_after(&stage(2, Some("x"), 6), &base), "writer");
    assert!(
        !staged_after(&stage(2, None, 6), &stage(2, None, 5)),
        "no writer"
    );
    assert!(!staged_after(&stage(2, Some("w"), -1), &base), "unstaged");
    assert!(!staged_after(
        &stage(2, Some("w"), 6),
        &stage(2, Some("w"), -1)
    ));
    // Zero is a stage.
    assert!(!staged_after(
        &stage(2, Some("w"), 0),
        &stage(2, Some("w"), -1)
    ));
    assert!(staged_after(
        &stage(2, Some("w"), 1),
        &stage(2, Some("w"), 0)
    ));
}

#[test]
fn the_newer_of_the_register_and_a_staged_commit_is_picked() {
    let register = stage(2, Some("w"), 5);
    let later = stage(2, Some("w"), 6);
    let older = stage(2, Some("w"), 4);
    assert_eq!(newer_than(&register, Some(&later)), &later);
    assert_eq!(newer_than(&register, Some(&older)), &register);
    assert_eq!(newer_than(&register, None), &register);
    assert_eq!(
        newer_than(&register, Some(&stage(2, Some("x"), 9))),
        &register,
        "another writer's is not ordered with it"
    );
    let unstaged = stage(2, Some("w"), -1);
    assert_eq!(newer_than(&unstaged, Some(&later)), &unstaged);
    let anonymous = stage(2, None, 5);
    assert_eq!(newer_than(&anonymous, Some(&stage(2, None, 6))), &anonymous);
}

#[test]
fn a_commit_is_newer_by_term_then_stage_then_generation() {
    let at = |term, writer: Option<&str>, seq, generation| {
        Stage::new(term, writer.map(str::to_owned), seq).with_generation(generation)
    };
    // The term decides first, whatever the rest say.
    assert!(newer(&at(3, Some("w"), 0, 1), &at(2, Some("w"), 9, 9)));
    assert!(!newer(&at(2, Some("w"), 9, 9), &at(3, Some("w"), 0, 1)));
    // In one term a later stage of one writer.
    assert!(newer(&at(2, Some("w"), 6, -1), &at(2, Some("w"), 5, -1)));
    assert!(!newer(&at(2, Some("w"), 5, -1), &at(2, Some("w"), 5, -1)));
    // Else a later generation, when both are known.
    assert!(newer(&at(2, Some("w"), -1, 6), &at(2, Some("x"), -1, 5)));
    assert!(!newer(&at(2, Some("w"), -1, 5), &at(2, Some("x"), -1, 5)));
    assert!(!newer(&at(2, None, -1, 6), &at(2, None, -1, -1)), "unknown");
    assert!(!newer(&at(2, None, -1, -1), &at(2, None, -1, 5)), "unknown");
    // Generation zero is a generation.
    assert!(newer(&at(2, None, -1, 1), &at(2, None, -1, 0)));
}

#[test]
fn a_commit_that_names_no_segments_file_has_no_generation_to_be_newer_by() {
    let none = stage_of(&commit(2, None, -1)).expect("a commit");
    let zero = stage_of(&commit_at(2, None, -1, "segments_0")).expect("a commit");
    let ten = stage_of(&commit_at(2, None, -1, "segments_a")).expect("a commit");
    assert!(!newer(&none, &zero), "no generation is not newer than zero");
    assert!(!newer(&zero, &none));
    assert!(newer(&ten, &zero), "base 36");
    assert!(stage_of(b"not a commit").is_none());
}

#[test]
fn a_later_stage_of_the_same_writer_and_term_replaces_the_register() {
    use oss_reader::commit_push::newer_commit;
    let register = commit(2, Some("w"), 4);
    let later = commit(2, Some("w"), 6);
    let asked = std::cell::RefCell::new(Vec::new());
    let chosen = newer_commit(&register, |writer| {
        asked.borrow_mut().push(writer.to_owned());
        Some(later.clone())
    });
    assert_eq!(chosen, Some(later));
    assert_eq!(*asked.borrow(), ["w"]);
    for answer in [
        commit(2, Some("x"), 6),
        commit(2, Some("w"), 4),
        commit(2, Some("w"), 3),
        commit(3, Some("w"), 6),
        b"not a commit".to_vec(),
    ] {
        assert_eq!(newer_commit(&register, |_| Some(answer.clone())), None);
    }
    assert_eq!(newer_commit(&register, |_| None), None);
}

#[test]
fn a_register_commit_that_was_not_staged_is_never_asked_about() {
    use oss_reader::commit_push::newer_commit;
    let ask = |_: &str| -> Option<Vec<u8>> { panic!("asked") };
    assert_eq!(newer_commit(&commit(2, Some("w"), -1), ask), None);
    assert_eq!(newer_commit(&commit(2, None, 4), ask), None);
    assert_eq!(newer_commit(b"not a commit", ask), None);
}
