//! docs/specs/coordinator-cat-tables: cat tables are drawn as the shell
//! draws them, and the cat and list index requests are decided as it decides
//! them.

use oss_coordinator::cat::health;
use oss_coordinator::cat::indices::{self, Paging};
use oss_coordinator::cat::table::{Page, Table, unsupported};
use oss_coordinator::cluster::health::{Health, Status};

fn table() -> Table<2> {
    let mut t = Table::new(["index", "pri"]);
    t.row(["a", "1"].map(str::to_owned));
    t.row(["longer", "12"].map(str::to_owned));
    t
}

#[test]
fn text_pads_to_the_widest_cell_and_headers_are_optional() {
    let t = table();
    assert_eq!(t.text(false, None), "a      1\nlonger 12\n");
    assert_eq!(t.text(true, None), "index  pri\na      1\nlonger 12\n");
    let mut wide = Table::new(["a", "very_long_header"]);
    wide.row(["x", "y"].map(str::to_owned));
    assert_eq!(wide.text(true, None), "a very_long_header\nx y\n");
    assert_eq!(wide.text(false, None), "x y\n");
    let mut astral = Table::new(["n", "v"]);
    astral.row(["\u{1F600}", "1"].map(str::to_owned));
    astral.row(["ab", "2"].map(str::to_owned));
    assert_eq!(astral.text(false, None), "\u{1F600} 1\nab 2\n");
    assert_eq!(Table::<1>::new(["h"]).text(false, None), "");
    assert_eq!(Table::<1>::new(["h"]).text(true, None), "h\n");
}

#[test]
fn a_missing_cell_is_null() {
    let mut t = Table::new(["a", "b"]);
    t.row_opt([Some("x".to_owned()), None]);
    assert_eq!(t.text(false, None), "x null\n");
    assert_eq!(t.json(None), r#"[{"a":"x","b":"null"}]"#);
}

#[test]
fn structured_is_strings_and_a_page_carries_its_token() {
    let t = table();
    assert_eq!(
        t.json(None),
        r#"[{"index":"a","pri":"1"},{"index":"longer","pri":"12"}]"#
    );
    assert_eq!(Table::<1>::new(["h"]).json(None), "[]");
    let more = Page {
        entity: "indices",
        next_token: Some("tok"),
    };
    let last = Page {
        entity: "indices",
        next_token: None,
    };
    assert_eq!(
        t.json(Some(more)),
        r#"{"next_token":"tok","indices":[{"index":"a","pri":"1"},{"index":"longer","pri":"12"}]}"#
    );
    assert_eq!(
        t.json(Some(last)),
        r#"{"next_token":null,"indices":[{"index":"a","pri":"1"},{"index":"longer","pri":"12"}]}"#
    );
    assert_eq!(
        t.text(false, Some(more)),
        "a      1\nlonger 12\nnext_token tok\n"
    );
    assert_eq!(
        t.text(false, Some(last)),
        "a      1\nlonger 12\nnext_token null\n"
    );
    let mut odd = Table::new(["k"]);
    odd.row(["q\"\\\n\t\u{1}é"].map(str::to_owned));
    assert_eq!(odd.json(None), r#"[{"k":"q\"\\\n\t\u0001é"}]"#);
}

#[test]
fn format_picks_text_or_structured() {
    let t = table();
    assert!(!t.answer(None, false, None).structured);
    assert!(!t.answer(Some("text"), false, None).structured);
    for f in ["json", "yaml", "cbor", "smile", ""] {
        let a = t.answer(Some(f), false, None);
        assert!(a.structured, "{f}");
        assert_eq!(a.body, t.json(None));
    }
    let a = t.answer(None, true, None);
    assert_eq!(a.body, t.text(true, None));
}

#[test]
fn h_and_s_are_refused_in_the_shells_words() {
    assert_eq!(unsupported(None, None), None);
    let h = unsupported(Some("index"), None).expect("h");
    assert_eq!(
        h,
        "'h' selects columns, which this implementation does not support; it answers with a fixed \
         column set, and returning that while accepting 'h' would hand back a table that is not the \
         one asked for"
    );
    let s = unsupported(None, Some("index")).expect("s");
    assert_eq!(
        s,
        "'s' sorts, which this implementation does not support; rows come back in a fixed order"
    );
    assert_eq!(unsupported(Some("x"), Some("y")).expect("both"), h);
}

#[test]
fn the_health_row_names_the_cluster_and_dashes_an_unasked_shard_count() {
    let mut h = Health {
        status: Status::Green,
        nodes: 3,
        active_shards: 7,
        dormant_shards: 2,
        ..Health::default()
    };
    assert_eq!(
        health::row("serverless", &h).text(true, None),
        "cluster    status node.total node.data shards dormant\nserverless green  3          3         -      -\n"
    );
    h.indices.push(("i".to_owned(), Default::default()));
    h.status = Status::Red;
    assert_eq!(health::row("c", &h).text(false, None), "c red 3 3 7 2\n");
}

#[test]
fn an_index_row_is_open_with_no_replicas() {
    let mut t = indices::table();
    indices::row(&mut t, "logs", "u-1", 4, 9);
    assert_eq!(
        t.text(true, None),
        "index uuid pri rep status mapping_version\nlogs  u-1  4   0   open   9\n"
    );
}

fn plan(
    listing: bool,
    index: &str,
    size: Option<&str>,
    token: Option<&str>,
    cat: Option<&str>,
) -> Result<Option<Paging>, (u16, &'static str, String)> {
    indices::plan(listing, index, size, token, cat).map_err(|r| (r.status, r.kind, r.reason))
}

#[test]
fn a_request_is_refused_in_order_and_walks_or_pages() {
    let unsupported = Some("no h");
    assert_eq!(
        plan(true, "*", Some("x"), Some("t"), unsupported),
        Err((501, "unsupported_cat_parameter", "no h".to_owned()))
    );
    assert_eq!(
        plan(false, "*", Some("5"), None, None),
        Err((
            400,
            "illegal_argument_exception",
            "_cat/indices is not paginated; _list/indices takes size and next_token".to_owned()
        ))
    );
    assert!(plan(false, "*", None, Some("t"), None).is_err());
    assert_eq!(plan(false, "a*", None, None, None), Ok(None));
    let size_msg = |s: &str| plan(true, "a*", Some(s), None, None).expect_err(s).2;
    assert_eq!(size_msg("x"), "size must be a number, got [x]");
    assert_eq!(size_msg(""), "size must be a number, got []");
    assert_eq!(
        size_msg("99999999999"),
        "size must be a number, got [99999999999]"
    );
    assert_eq!(size_msg("0"), "size must be between 1 and [5000], got [0]");
    assert_eq!(
        size_msg("5001"),
        "size must be between 1 and [5000], got [5001]"
    );
    assert_eq!(
        size_msg("-3"),
        "size must be between 1 and [5000], got [-3]"
    );
    for ok in ["1", "5000", "+7"] {
        assert!(plan(true, "a*", Some(ok), None, None).is_ok(), "{ok}");
    }
    // Named indices and lists are one page; a token needs one prefix.
    assert_eq!(
        plan(true, "a,b", Some("10"), None, None),
        Ok(Some(Paging {
            prefix: None,
            after: None,
            size: 10
        }))
    );
    assert_eq!(
        plan(true, "a*,b*", None, Some("t"), None)
            .expect_err("list")
            .2,
        "next_token resumes a walk over one prefix pattern, and [a*,b*] is not one"
    );
    assert_eq!(
        plan(true, "name", None, Some("t"), None)
            .expect_err("name")
            .2,
        "next_token resumes a walk over one prefix pattern, and [name] is not one"
    );
    // A prefix walk, and the bare form meaning every index.
    assert_eq!(
        plan(true, " logs-* ", None, None, None),
        Ok(Some(Paging {
            prefix: Some("logs-".to_owned()),
            after: None,
            size: 500
        }))
    );
    // Java's trim strips only characters up to a space, not other Unicode space.
    assert_eq!(
        plan(true, "\u{1}p*\n", None, None, None),
        Ok(Some(Paging {
            prefix: Some("p".to_owned()),
            after: None,
            size: 500
        }))
    );
    assert_eq!(
        plan(true, "\u{a0}p*", None, None, None),
        Ok(Some(Paging {
            prefix: Some("\u{a0}p".to_owned()),
            after: None,
            size: 500
        }))
    );
    assert_eq!(
        plan(true, "*", Some("20"), None, None),
        Ok(Some(Paging {
            prefix: Some(String::new()),
            after: None,
            size: 20
        }))
    );
    assert_eq!(
        plan(true, "a*b*", None, None, None),
        Ok(Some(Paging {
            prefix: None,
            after: None,
            size: 500
        }))
    );
}

#[test]
fn a_token_round_trips_and_anything_else_is_tainted() {
    let tainted = "Parameter [next_token] has been tainted and is incorrect. Please provide a valid [next_token].";
    let token = indices::token("logs-", "logs-0042");
    assert!(!token.contains('=') && !token.contains('+') && !token.contains('/'));
    assert_eq!(token, "MQpsb2dzLQpsb2dzLTAwNDI");
    assert_eq!(
        plan(true, "logs-*", Some("5"), Some(&token), None),
        Ok(Some(Paging {
            prefix: Some("logs-".to_owned()),
            after: Some("logs-0042".to_owned()),
            size: 5
        }))
    );
    let short = indices::token("p", "x");
    assert_eq!(short, "MQpwCng");
    for ok in [format!("{short}="), short.clone()] {
        assert!(plan(true, "p*", None, Some(&ok), None).is_ok(), "{ok}");
    }
    let padded = format!("{token}=");
    assert!(plan(true, "logs-*", None, Some(&padded), None).is_ok());
    let other = plan(true, "other-*", None, Some(&token), None).expect_err("prefix");
    assert_eq!((other.0, other.1), (400, "illegal_argument_exception"));
    assert_eq!(other.2, tainted);
    for bad in [
        "!!",
        "a+b/",
        "MQpwCng==x",
        "MQpwCng==",
        "bm90YXRva2Vu",
        "MXAKeA",
    ] {
        assert_eq!(
            plan(true, "p*", None, Some(bad), None).expect_err(bad).2,
            tainted,
            "{bad}"
        );
    }
    // Two padding characters, and padding that does not fit the length.
    let two = indices::token("", "z");
    assert_eq!(two, "MQoKeg");
    assert!(plan(true, "*", None, Some(&format!("{two}==")), None).is_ok());
    for bad in ["MQoKeg=", "MQoKeg===", "MQpwCn=", "MQpwCngAA"] {
        assert_eq!(
            plan(true, "p*", None, Some(bad), None).expect_err(bad).2,
            tainted,
            "{bad}"
        );
    }
    // The bare form's token carries an empty prefix.
    let bare = indices::token("", "z");
    assert_eq!(
        plan(true, "*", None, Some(&bare), None),
        Ok(Some(Paging {
            prefix: Some(String::new()),
            after: Some("z".to_owned()),
            size: 500
        }))
    );
}
