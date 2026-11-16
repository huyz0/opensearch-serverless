//! Writer calls one at a time: escaping, building, refusals, framing, outputs.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

use chunkjson::{Config, Event, Framing, IoOutput, Misuse, Output, Text, WriteError, Writer};

fn written(f: impl FnOnce(&mut Writer, &mut Vec<u8>)) -> String {
    let mut w = Writer::new(Config::default());
    let mut out = Vec::new();
    f(&mut w, &mut out);
    w.check_complete().unwrap();
    String::from_utf8(out).unwrap()
}

/// AC-4
#[test]
fn strings_are_escaped_as_jackson_escapes_them() {
    for b in 0u8..0x20 {
        let s = char::from(b).to_string();
        let want = match b {
            0x08 => "\"\\b\"".to_owned(),
            0x09 => "\"\\t\"".to_owned(),
            0x0a => "\"\\n\"".to_owned(),
            0x0c => "\"\\f\"".to_owned(),
            0x0d => "\"\\r\"".to_owned(),
            _ => format!("\"\\u00{b:02X}\""),
        };
        assert_eq!(written(|w, o| w.string(&s, o).unwrap()), want, "{b:#x}");
    }
    assert_eq!(
        written(|w, o| w.string("a\"b\\c", o).unwrap()),
        r#""a\"b\\c""#
    );
    assert_eq!(
        written(|w, o| w.string("/\x7fé€😀 ~", o).unwrap()),
        "\"/\x7fé€😀 ~\""
    );
    assert_eq!(
        written(|w, o| w.string("long run of plain text\x1fthen more", o).unwrap()),
        "\"long run of plain text\\u001Fthen more\""
    );
}

#[test]
fn building_calls_write_compact_json() {
    let s = written(|w, o| {
        w.begin_object(o).unwrap();
        w.key("a", o).unwrap();
        w.begin_array(o).unwrap();
        w.u64(u64::MAX, o).unwrap();
        w.i64(i64::MIN, o).unwrap();
        w.i64(0, o).unwrap();
        w.i64(42, o).unwrap();
        w.number("-1.5e3", o).unwrap();
        w.bool(true, o).unwrap();
        w.bool(false, o).unwrap();
        w.null(o).unwrap();
        w.begin_object(o).unwrap();
        w.end_object(o).unwrap();
        w.end_array(o).unwrap();
        w.key("b", o).unwrap();
        w.string("", o).unwrap();
        w.end_object(o).unwrap();
    });
    assert_eq!(
        s,
        r#"{"a":[18446744073709551615,-9223372036854775808,0,42,-1.5e3,true,false,null,{}],"b":""}"#
    );
}

/// Runs `call`, which must be refused with `want` and write nothing to `out`.
macro_rules! refused {
    ($out:ident, $call:expr, $want:expr) => {{
        let before = $out.len();
        match $call {
            Err(WriteError::Misuse(m)) => assert_eq!(m, $want),
            other => panic!("expected {:?}, got {other:?}", $want),
        }
        assert_eq!($out.len(), before, "a refused call wrote");
    }};
}

#[test]
fn misuse_is_refused_with_its_kind_and_writes_nothing() {
    let mut w = Writer::new(Config {
        max_depth: 2,
        ..Config::default()
    });
    let mut o = Vec::new();
    refused!(o, w.key("k", &mut o), Misuse::KeyNotAllowed);
    refused!(o, w.end_array(&mut o), Misuse::EndNotAllowed);
    assert_eq!(
        w.check_complete(),
        Err(Misuse::Unfinished),
        "nothing written yet"
    );
    w.begin_object(&mut o).unwrap();
    refused!(o, w.null(&mut o), Misuse::ValueNotAllowed);
    refused!(o, w.end_array(&mut o), Misuse::EndNotAllowed);
    w.key("k", &mut o).unwrap();
    refused!(o, w.key("k2", &mut o), Misuse::KeyNotAllowed);
    refused!(o, w.end_object(&mut o), Misuse::EndNotAllowed);
    refused!(o, w.number("01", &mut o), Misuse::InvalidNumber);
    w.begin_array(&mut o).unwrap();
    refused!(o, w.begin_array(&mut o), Misuse::TooDeep);
    let open = Text {
        fragment: "ab",
        first: true,
        last: false,
    };
    w.event(Event::String(open), &mut o).unwrap();
    refused!(o, w.null(&mut o), Misuse::StringOpen);
    refused!(o, w.end_array(&mut o), Misuse::StringOpen);
    refused!(o, w.key("k", &mut o), Misuse::NotAContinuation);
    refused!(
        o,
        w.event(Event::String(open), &mut o),
        Misuse::NotAContinuation
    );
    let key_piece = Text {
        fragment: "c",
        first: false,
        last: true,
    };
    refused!(
        o,
        w.event(Event::Key(key_piece), &mut o),
        Misuse::NotAContinuation
    );
    assert_eq!(
        w.check_complete(),
        Err(Misuse::Unfinished),
        "a string is open"
    );
    w.event(Event::String(key_piece), &mut o).unwrap();
    refused!(
        o,
        w.event(Event::String(key_piece), &mut o),
        Misuse::NotAContinuation
    );
    assert_eq!(
        w.check_complete(),
        Err(Misuse::Unfinished),
        "containers are open"
    );
    w.end_array(&mut o).unwrap();
    w.end_object(&mut o).unwrap();
    assert_eq!(w.check_complete(), Ok(()));
    refused!(o, w.null(&mut o), Misuse::ValueNotAllowed);
    assert_eq!(String::from_utf8(o).unwrap(), r#"{"k":["abc"]}"#);
}

#[test]
fn framed_writers_put_each_value_on_its_own_line() {
    for framing in [Framing::Ndjson, Framing::Concatenated] {
        let mut w = Writer::new(Config::with_framing(framing));
        let mut o = Vec::new();
        assert_eq!(w.check_complete(), Ok(()), "an empty stream is complete");
        w.null(&mut o).unwrap();
        w.begin_array(&mut o).unwrap();
        w.end_array(&mut o).unwrap();
        w.string("x", &mut o).unwrap();
        assert_eq!(w.check_complete(), Ok(()));
        assert_eq!(o, b"null\n[]\n\"x\"\n");
    }
}

#[test]
fn stream_events_write_nothing() {
    let mut w = Writer::new(Config::default());
    let mut o = Vec::new();
    w.event(Event::NeedMore, &mut o).unwrap();
    w.event(Event::BeginArray, &mut o).unwrap();
    w.event(Event::EndArray, &mut o).unwrap();
    w.event(Event::EndDocument { start: 0, end: 2 }, &mut o)
        .unwrap();
    w.event(Event::Eof, &mut o).unwrap();
    w.event(Event::Bool(true), &mut o).unwrap_err();
    assert_eq!(o, b"[]");
}

struct Broken(usize);

impl Output for Broken {
    type Error = &'static str;
    fn write(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.0 < bytes.len() {
            return Err("full");
        }
        self.0 -= bytes.len();
        Ok(())
    }
}

#[test]
fn an_output_error_is_returned_and_the_writer_then_refuses() {
    let mut w = Writer::new(Config::default());
    let mut o = Broken(3);
    w.begin_array(&mut o).unwrap();
    assert!(matches!(
        w.string("long", &mut o),
        Err(WriteError::Output("full"))
    ));
    assert!(matches!(
        w.null(&mut o),
        Err(WriteError::Misuse(Misuse::Failed))
    ));
    assert_eq!(w.check_complete(), Err(Misuse::Failed));
}

#[test]
fn io_output_writes_through_std_io() {
    let mut w = Writer::new(Config::default());
    let mut o = IoOutput(Vec::new());
    w.begin_array(&mut o).unwrap();
    w.string("é", &mut o).unwrap();
    w.end_array(&mut o).unwrap();
    assert_eq!(o.0, "[\"é\"]".as_bytes());
    let mut full = IoOutput(&mut [0u8; 1][..]);
    let mut w = Writer::new(Config::default());
    assert!(matches!(
        w.string("ab", &mut full),
        Err(WriteError::Output(_))
    ));
}

#[test]
fn errors_say_what_was_refused() {
    let e: WriteError<std::io::Error> = Misuse::TooDeep.into();
    assert_eq!(
        e.to_string(),
        "refused: nesting is deeper than the configured limit"
    );
    assert!(std::error::Error::source(&e).is_some());
    let e: WriteError<std::io::Error> = WriteError::Output(std::io::Error::other("full"));
    assert_eq!(e.to_string(), "output failed: full");
    assert!(std::error::Error::source(&e).is_some());
    for m in [
        Misuse::KeyNotAllowed,
        Misuse::ValueNotAllowed,
        Misuse::EndNotAllowed,
        Misuse::StringOpen,
        Misuse::NotAContinuation,
        Misuse::InvalidNumber,
        Misuse::Unfinished,
        Misuse::Failed,
    ] {
        assert!(!m.to_string().is_empty());
    }
}
