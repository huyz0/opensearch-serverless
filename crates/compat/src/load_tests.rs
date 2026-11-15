use std::fs;
use std::path::PathBuf;

use super::*;

/// A fresh directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("oss-compat-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture_text(name: &str) -> Vec<u8> {
    format!(r#"{{"name": "{name}", "steps": [{{"name": "root", "request": {{"method": "GET", "path": "/"}}}}]}}"#)
        .into_bytes()
}

#[test]
fn a_file_of_exactly_the_limit_loads_and_one_byte_more_is_refused() {
    let dir = Scratch::new("limit");
    let at_limit = dir.file("at.json", &vec![b' '; MAX_FIXTURE_BYTES]);
    assert_eq!(
        read_bounded(&at_limit, MAX_FIXTURE_BYTES).unwrap().len(),
        MAX_FIXTURE_BYTES
    );
    let past = dir.file("past.json", &vec![b' '; MAX_FIXTURE_BYTES + 1]);
    let e = read_bounded(&past, MAX_FIXTURE_BYTES).unwrap_err();
    assert!(
        matches!(
            e,
            LoadError::TooLarge {
                limit: MAX_FIXTURE_BYTES,
                ..
            }
        ),
        "{e}"
    );
    assert!(
        e.to_string()
            .contains("larger than the 1048576-byte fixture limit")
    );
    let e = load_file(&past).unwrap_err();
    assert!(matches!(e, LoadError::TooLarge { .. }), "{e}");
}

#[test]
fn small_limits_hold_across_chunk_boundaries() {
    let dir = Scratch::new("chunks");
    let path = dir.file("a.json", &vec![b'x'; CHUNK + 1]);
    assert_eq!(read_bounded(&path, CHUNK + 1).unwrap().len(), CHUNK + 1);
    assert!(matches!(
        read_bounded(&path, CHUNK),
        Err(LoadError::TooLarge { .. })
    ));
    assert!(matches!(
        read_bounded(&path, 0),
        Err(LoadError::TooLarge { .. })
    ));
}

#[test]
fn a_missing_file_is_an_io_error_naming_it() {
    let e = load_file(Path::new("/nonexistent/fixture.json")).unwrap_err();
    assert!(matches!(e, LoadError::Io { .. }));
    assert!(
        e.to_string().starts_with("/nonexistent/fixture.json: "),
        "{e}"
    );
}

#[test]
fn an_invalid_fixture_names_the_file_and_the_field() {
    let dir = Scratch::new("invalid");
    let path = dir.file("bad.json", br#"{"name": "x", "steps": []}"#);
    let e = load_file(&path).unwrap_err();
    assert!(matches!(e, LoadError::Invalid { .. }));
    assert!(
        e.to_string()
            .ends_with("bad.json: steps: a fixture needs at least one step"),
        "{e}"
    );
}

#[test]
fn a_directory_loads_every_json_file_sorted_and_skips_the_rest() {
    let dir = Scratch::new("dir");
    dir.file("search/b.json", &fixture_text("b"));
    dir.file("index/a.json", &fixture_text("a"));
    dir.file("README.md", b"# not a fixture");
    dir.file("record.py", b"print()");
    let loaded = load_dir(&dir.0).unwrap();
    let names: Vec<&str> = loaded.iter().map(|(_, f)| f.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(loaded[0].0.ends_with("index/a.json"));
}

#[test]
fn a_directory_with_a_bad_fixture_fails_to_load() {
    let dir = Scratch::new("dirbad");
    dir.file("a.json", &fixture_text("a"));
    dir.file("b.json", b"{");
    assert!(matches!(load_dir(&dir.0), Err(LoadError::Invalid { .. })));
}

#[test]
fn the_file_count_and_nesting_depth_are_bounded() {
    let dir = Scratch::new("bounds");
    dir.file("a.json", &fixture_text("a"));
    dir.file("b.json", &fixture_text("b"));
    let mut out = Vec::new();
    assert!(collect(&dir.0, 0, 2, &mut out).is_ok());
    let mut out = Vec::new();
    let e = collect(&dir.0, 0, 1, &mut out).unwrap_err();
    assert!(matches!(e, LoadError::TooMany { .. }));
    assert!(e.to_string().contains("more than 10000 fixtures"), "{e}");

    let deep = Scratch::new("deep");
    deep.file("1/2/3/4/5/x.json", &fixture_text("x"));
    assert!(matches!(load_dir(&deep.0), Err(LoadError::TooMany { .. })));
    let shallow = Scratch::new("shallow");
    shallow.file("1/2/3/4/x.json", &fixture_text("x"));
    assert_eq!(load_dir(&shallow.0).unwrap().len(), 1);
}

#[test]
fn a_missing_directory_is_an_io_error() {
    assert!(matches!(
        load_dir(Path::new("/nonexistent/fixtures")),
        Err(LoadError::Io { .. })
    ));
}
