//! A shard indexes a field with the module its mapping pins, and refuses
//! the write rather than fall back to the built-in analyzer.

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_ext::analyzer::{Analyze, Token};
use oss_ext::{CallError, Status};
use oss_writer::index::knn::open_writer;
use oss_writer::index::{Expect, Refused, Shard};
use oss_writer::mapping::{Mapping, ParseOptions};

const MAPPING: &[u8] = br#"{"properties":{"t":{"type":"text","analyzer":"ext_shout"}}}"#;

struct Shout;

impl Analyze for Shout {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        Ok(vec![Token {
            term: text.to_uppercase(),
            start: 0,
            end: u32::try_from(text.encode_utf16().count()).expect("small"),
            increment: 1,
        }])
    }
}

struct Broken;

impl Analyze for Broken {
    fn analyze(&self, _: &str) -> Result<Vec<Token>, CallError> {
        Err(CallError::from_status(Status::Limit, b"fuel"))
    }
}

fn shard<'d>(dir: &'d ByteBuffersDirectory, module: Arc<dyn Analyze>) -> Shard<'d> {
    shard_over(dir, module, MAPPING)
}

fn shard_over<'d>(
    dir: &'d ByteBuffersDirectory,
    module: Arc<dyn Analyze>,
    mapping: &[u8],
) -> Shard<'d> {
    let mapping = Mapping::parse_pinned(mapping, ParseOptions::default()).expect("mapping");
    let (writer, derived) = open_writer(dir, &mapping).expect("writer");
    Shard::new(dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived)
        .with_module("ext_shout", module)
}

// docs/specs/extension-analyzer AC-10
#[test]
fn a_pinned_field_is_indexed_with_the_modules_terms() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, Arc::new(Shout));
    shard
        .index("1", br#"{"t":"Hello World"}"#, false, &Expect::default())
        .expect("indexed");
    shard.refresh().expect("refresh");
    let reader = DirectoryReader::open(&dir).expect("open");
    let freq = |term: &str| {
        reader.segment_readers()[0]
            .doc_freq("t", term.as_bytes())
            .expect("freq")
    };
    assert_eq!(freq("HELLO WORLD"), 1);
    assert_eq!(freq("hello"), 0);
}

// docs/specs/extension-analyzer AC-8: no result built without the module
#[test]
fn a_module_that_fails_refuses_the_write() {
    let dir = ByteBuffersDirectory::new();
    let mut shard = shard(&dir, Arc::new(Broken));
    let e = shard
        .index("1", br#"{"t":"Hello"}"#, false, &Expect::default())
        .expect_err("refused");
    let Refused::Mapping(e) = e else {
        panic!("{e:?}")
    };
    assert_eq!(e.ty, "extension_failed", "a limit is the input's, 400");
}

struct Failing(fn() -> CallError);

impl Analyze for Failing {
    fn analyze(&self, _: &str) -> Result<Vec<Token>, CallError> {
        Err((self.0)())
    }
}

// docs/specs/extension-analyzer AC-2, AC-16
#[test]
fn a_failure_is_typed_by_what_a_caller_can_do() {
    for (make, ty) in [
        (CallError::busy as fn() -> CallError, "extension_busy"),
        (|| CallError::malformed(9), "extension_malformed"),
        (|| CallError::trap("unreachable"), "extension_failed"),
    ] {
        let dir = ByteBuffersDirectory::new();
        let mut shard = shard(&dir, Arc::new(Failing(make)));
        let Err(Refused::Mapping(e)) =
            shard.index("1", br#"{"t":"Hello"}"#, false, &Expect::default())
        else {
            panic!("refused")
        };
        assert_eq!(e.ty, ty);
    }
}

/// One token a word, upper-cased, with the word's own offsets.
struct Words;

impl Analyze for Words {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let mut out = Vec::new();
        let mut at = 0_u32;
        for word in text.split(' ') {
            let n = u32::try_from(word.encode_utf16().count()).expect("small");
            if n > 0 {
                out.push(Token {
                    term: word.to_uppercase(),
                    start: at,
                    end: at.saturating_add(n),
                    increment: 1,
                });
            }
            at = at.saturating_add(n).saturating_add(1);
        }
        Ok(out)
    }
}

const SOURCES: [&str; 4] = [
    r#"{"t":"the quick brown fox"}"#,
    r#"{"t":"jumps over the lazy dog"}"#,
    r#"{"t":"quick quick quick"}"#,
    r#"{"t":"the end"}"#,
];

fn terms_of(dir: &ByteBuffersDirectory) -> Vec<(String, i32)> {
    let reader = DirectoryReader::open(dir).expect("open");
    let seg = &reader.segment_readers()[0];
    [
        "THE", "QUICK", "BROWN", "FOX", "JUMPS", "OVER", "LAZY", "DOG", "END", "the", "quick",
    ]
    .iter()
    .map(|t| {
        (
            (*t).to_owned(),
            seg.doc_freq("t", t.as_bytes()).expect("freq"),
        )
    })
    .collect()
}

// docs/specs/extension-analyzer AC-12: replay reproduces what the live write indexed
#[test]
fn a_replay_indexes_the_same_terms_as_the_live_write() {
    let live_dir = ByteBuffersDirectory::new();
    let mut live = shard(&live_dir, Arc::new(Words));
    for (n, source) in SOURCES.iter().enumerate() {
        live.index(
            &format!("d{n}"),
            source.as_bytes(),
            false,
            &Expect::default(),
        )
        .expect("indexed");
    }
    live.refresh().expect("refresh");

    let replay_dir = ByteBuffersDirectory::new();
    let mut replayed = shard(&replay_dir, Arc::new(Words));
    for (n, source) in SOURCES.iter().enumerate() {
        let seq = i64::try_from(n).expect("small");
        let record =
            oss_format::WalRecord::index(format!("d{n}"), (*source).to_owned(), seq, 1, seq + 1);
        assert!(replayed.replay(&record).expect("replayed"));
    }
    replayed.refresh().expect("refresh");

    let (a, b) = (terms_of(&live_dir), terms_of(&replay_dir));
    assert_eq!(a, b, "the same documents frequency for every term");
    assert_eq!(a.iter().find(|t| t.0 == "QUICK").expect("term").1, 2);
    assert_eq!(a.iter().find(|t| t.0 == "quick").expect("term").1, 0);
}

/// Every (doc, position, start, end) a term has, read through the postings.
fn postings_of(dir: &ByteBuffersDirectory, term: &str) -> Vec<(i32, i32, i32, i32)> {
    use lucene_search::reader::{LeafReader, PostingsFlags};
    let reader = DirectoryReader::open(dir).expect("open");
    let seg = &reader.segment_readers()[0];
    let terms = seg.terms("t").expect("terms").expect("field");
    let mut it = terms.iterator().expect("iterator");
    if !it.try_seek_exact(term.as_bytes()).expect("seek") {
        return Vec::new();
    }
    let mut postings = it.postings(PostingsFlags::Offsets).expect("postings");
    let mut out = Vec::new();
    while postings.next_doc().expect("doc") != i32::MAX {
        for _ in 0..postings.freq() {
            let pos = postings.next_position().expect("position");
            out.push((
                postings.doc_id(),
                pos,
                postings.start_offset(),
                postings.end_offset(),
            ));
        }
    }
    out
}

// docs/specs/extension-analyzer AC-12: replay reproduces positions and offsets too
#[test]
fn a_replay_indexes_the_same_positions_and_offsets_as_the_live_write() {
    let mapping: &[u8] =
        br#"{"properties":{"t":{"type":"text","analyzer":"ext_shout","index_options":"offsets"}}}"#;
    let live_dir = ByteBuffersDirectory::new();
    let mut live = shard_over(&live_dir, Arc::new(Words), mapping);
    let replay_dir = ByteBuffersDirectory::new();
    let mut replayed = shard_over(&replay_dir, Arc::new(Words), mapping);
    for (n, source) in SOURCES.iter().enumerate() {
        live.index(
            &format!("d{n}"),
            source.as_bytes(),
            false,
            &Expect::default(),
        )
        .expect("indexed");
        let seq = i64::try_from(n).expect("small");
        let record =
            oss_format::WalRecord::index(format!("d{n}"), (*source).to_owned(), seq, 1, seq + 1);
        assert!(replayed.replay(&record).expect("replayed"));
    }
    live.refresh().expect("refresh");
    replayed.refresh().expect("refresh");
    for term in ["THE", "QUICK", "FOX", "END", "DOG"] {
        let (a, b) = (postings_of(&live_dir, term), postings_of(&replay_dir, term));
        assert!(!a.is_empty(), "{term} is indexed");
        assert_eq!(a, b, "{term}: positions and offsets");
    }
    assert_eq!(postings_of(&live_dir, "QUICK")[0], (0, 1, 4, 9));
}

/// A Lucene file without its codec header (magic, name, version, segment id,
/// suffix) and its 16-byte footer, which carry what differs between two
/// writers of the same segment: the id and the checksum over it.
fn body(file: &[u8]) -> &[u8] {
    // The magic, the name, the version and the segment id, then the suffix.
    let after_id = [4, 1, usize::from(file[4]), 4, 16]
        .iter()
        .fold(0_usize, |a, n| a.saturating_add(*n));
    let at = after_id
        .saturating_add(1)
        .saturating_add(usize::from(file[after_id]));
    &file[at..file.len().saturating_sub(16)]
}

// docs/specs/extension-analyzer AC-12: replay reproduces the postings files
#[test]
fn a_replay_writes_the_same_postings_files_as_the_live_write() {
    use lucene_store::Directory;
    let mapping: &[u8] =
        br#"{"properties":{"t":{"type":"text","analyzer":"ext_shout","index_options":"offsets"}}}"#;
    let live_dir = ByteBuffersDirectory::new();
    let mut live = shard_over(&live_dir, Arc::new(Words), mapping);
    let replay_dir = ByteBuffersDirectory::new();
    let mut replayed = shard_over(&replay_dir, Arc::new(Words), mapping);
    for (n, source) in SOURCES.iter().enumerate() {
        live.index(
            &format!("d{n}"),
            source.as_bytes(),
            false,
            &Expect::default(),
        )
        .expect("indexed");
        let seq = i64::try_from(n).expect("small");
        let record =
            oss_format::WalRecord::index(format!("d{n}"), (*source).to_owned(), seq, 1, seq + 1);
        assert!(replayed.replay(&record).expect("replayed"));
    }
    live.refresh().expect("refresh");
    replayed.refresh().expect("refresh");
    let find = |dir: &ByteBuffersDirectory, ext: &str| {
        let name = dir
            .list_all()
            .expect("lists")
            .into_iter()
            .find(|f| f.ends_with(ext))
            .unwrap_or_else(|| panic!("a {ext} file"));
        dir.open(&name).expect("opens").as_ref().to_vec()
    };
    for ext in [".doc", ".pos", ".tim", ".tip"] {
        let (a, b) = (find(&live_dir, ext), find(&replay_dir, ext));
        assert!(!body(&a).is_empty(), "{ext} has content");
        assert_eq!(body(&a), body(&b), "{ext}: the same bytes after the header");
    }
}

/// A module that takes `each` for a call and may spend `budget` on a document.
struct Slow {
    each: std::time::Duration,
    budget: Option<std::time::Duration>,
}

impl Analyze for Slow {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        std::thread::sleep(self.each);
        Shout.analyze(text)
    }

    fn item_budget(&self) -> Option<std::time::Duration> {
        self.budget
    }
}

// docs/specs/extension-analyzer AC-11: a document whose calls take more than
// its budget between them is refused, and the next document is not affected.
#[test]
fn a_document_with_many_slow_calls_is_refused_and_the_next_is_not() {
    let ms = std::time::Duration::from_millis;
    let many = br#"{"t":["a","b","c","d","e","f","g","h"]}"#;
    let few = br#"{"t":["a","b"]}"#;

    let dir = ByteBuffersDirectory::new();
    let slow = Slow {
        each: ms(30),
        budget: Some(ms(100)),
    };
    let mut first = shard(&dir, Arc::new(slow));
    let Err(Refused::Mapping(e)) = first.index("1", many, false, &Expect::default()) else {
        panic!("eight 30 ms calls passed a 100 ms budget")
    };
    assert_eq!(e.ty, "extension_failed", "{e:?}");
    assert!(first.index("2", few, false, &Expect::default()).is_ok());

    // No budget (a replay): the same document passes.
    let dir = ByteBuffersDirectory::new();
    let free = Slow {
        each: ms(30),
        budget: None,
    };
    let mut second = shard(&dir, Arc::new(free));
    assert!(second.index("1", many, false, &Expect::default()).is_ok());
}

/// What the built-in `standard` analyzer makes of ordinary words: runs of
/// letters and digits, lower-cased, with offsets in UTF-16 units.
struct Golden;

impl Analyze for Golden {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let mut out = Vec::new();
        let mut word = String::new();
        let mut start = 0_u32;
        let mut at = 0_u32;
        let flush = |word: &mut String, start: u32, at: u32, out: &mut Vec<Token>| {
            if !word.is_empty() {
                out.push(Token {
                    term: word.to_lowercase(),
                    start,
                    end: at,
                    increment: 1,
                });
                word.clear();
            }
        };
        for c in text.chars() {
            let units = u32::try_from(c.len_utf16()).expect("one or two");
            if c.is_alphanumeric() {
                if word.is_empty() {
                    start = at;
                }
                word.push(c);
            } else {
                flush(&mut word, start, at, &mut out);
            }
            at = at.saturating_add(units);
        }
        flush(&mut word, start, at, &mut out);
        Ok(out)
    }
}

// docs/specs/extension-analyzer AC-10: a module that reimplements `standard` writes what it writes
#[test]
fn a_golden_module_writes_the_files_the_built_in_analyzer_writes() {
    use lucene_store::Directory;
    let builtin: &[u8] = br#"{"properties":{"t":{"type":"text","index_options":"offsets"}}}"#;
    let pinned: &[u8] =
        br#"{"properties":{"t":{"type":"text","analyzer":"ext_shout","index_options":"offsets"}}}"#;
    // Over 64 values, with astral letters before later words, so the offsets
    // after them differ between UTF-8 bytes and UTF-16 units, and a value
    // with no word at all, which still takes a position gap.
    let values: Vec<String> = (0..70)
        .map(|n| match n % 5 {
            0 => format!("Quick brown {n}"),
            1 => format!("\u{1D4B3}\u{1D4B4} Fox, jumps-over {n}"),
            2 => "!!! ...".to_owned(),
            3 => format!("The  LAZY dog\u{1D4B3} {n}"),
            _ => "caf\u{e9} \u{e9}t\u{e9}".to_owned(),
        })
        .collect();
    let doc = |values: &[String]| serde_json::json!({ "t": values, "u": "x" }).to_string();
    let source = doc(&values);
    let single = serde_json::json!({ "t": "Single value text" }).to_string();

    let built_dir = ByteBuffersDirectory::new();
    let mut built = shard_over_plain(&built_dir, builtin);
    let pinned_dir = ByteBuffersDirectory::new();
    let mut module = shard_over(&pinned_dir, Arc::new(Golden), pinned);
    for (n, s) in [&source, &single, &source, &single].into_iter().enumerate() {
        let id = format!("d{n}");
        built
            .index(&id, s.as_bytes(), false, &Expect::default())
            .expect("indexed");
        module
            .index(&id, s.as_bytes(), false, &Expect::default())
            .expect("indexed");
    }
    built.refresh().expect("refresh");
    module.refresh().expect("refresh");

    let find = |dir: &ByteBuffersDirectory, ext: &str| {
        let name = dir
            .list_all()
            .expect("lists")
            .into_iter()
            .find(|f| f.ends_with(ext))
            .unwrap_or_else(|| panic!("a {ext} file"));
        dir.open(&name).expect("opens").as_ref().to_vec()
    };
    // Terms, positions, offsets, and the norms (the field length per document).
    for ext in [".doc", ".pos", ".tim", ".tip", ".nvd"] {
        let (a, b) = (find(&built_dir, ext), find(&pinned_dir, ext));
        assert!(!body(&a).is_empty(), "{ext} has content");
        assert_eq!(
            body(&a),
            body(&b),
            "{ext}: the module wrote what the built-in did"
        );
    }
    // A spot check that the offsets are UTF-16 units: the first "fox" follows
    // the 13 units of the first value, the offset gap of 1, and two astral
    // letters and a space (5 units, 9 bytes), so it starts at 19 and not 23.
    assert_eq!(postings_of(&pinned_dir, "fox")[0].2, 19);
}

fn shard_over_plain<'d>(dir: &'d ByteBuffersDirectory, mapping: &[u8]) -> Shard<'d> {
    let mapping = Mapping::parse_pinned(mapping, ParseOptions::default()).expect("mapping");
    let (writer, derived) = open_writer(dir, &mapping).expect("writer");
    Shard::new(dir, writer, mapping, 1, Box::new(|| 0))
        .expect("shard")
        .with_derived_source(derived)
}
