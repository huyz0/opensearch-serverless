//! The analyzer point's codec (docs/specs/extension-analyzer AC-1, AC-2, AC-9).

use oss_ext::analyzer::{
    MAX_BLOCK_TEXT_BYTES, MAX_BLOCK_TEXTS, MAX_OUTPUT_BYTES, MAX_TERM_BYTES, MAX_TOKENS, Token,
    decode_block, encode_block, split_blocks,
};
use oss_ext::{CallError, FailureKind};

fn word(out: &mut Vec<u8>, w: u32) {
    out.extend_from_slice(&w.to_le_bytes());
}

/// One text's tokens as a module writes them: `(term, start, end, increment)`.
fn tokens(out: &mut Vec<u8>, toks: &[(&[u8], u32, u32, u32)]) {
    word(out, toks.len() as u32);
    for (t, s, e, i) in toks {
        word(out, t.len() as u32);
        out.extend_from_slice(t);
        word(out, *s);
        word(out, *e);
        word(out, *i);
    }
}

fn one(texts: &[&str], toks: &[(&[u8], u32, u32, u32)]) -> Result<Vec<Vec<Token>>, CallError> {
    let mut out = Vec::new();
    tokens(&mut out, toks);
    decode_block(texts, &out)
}

fn malformed(r: Result<Vec<Vec<Token>>, CallError>) -> String {
    let e = r.expect_err("refused");
    assert_eq!((e.kind(), e.http_status()), (FailureKind::Malformed, 502));
    e.detail().to_owned()
}

// docs/specs/extension-analyzer AC-1
#[test]
fn a_block_is_encoded_with_counts_and_lengths() {
    let b = encode_block(&["ab", "é"]).expect("encodes");
    let mut want = Vec::new();
    word(&mut want, 2);
    word(&mut want, 2);
    want.extend_from_slice(b"ab");
    word(&mut want, 2);
    want.extend_from_slice("é".as_bytes());
    assert_eq!(b, want);
}

#[test]
fn a_block_of_no_texts_or_too_many_is_refused() {
    let e = encode_block(&[]).expect_err("empty");
    assert_eq!(e.kind(), FailureKind::Limit);
    let many = vec!["x"; MAX_BLOCK_TEXTS + 1];
    assert_eq!(encode_block(&many).unwrap_err().kind(), FailureKind::Limit);
    assert!(encode_block(&many[..MAX_BLOCK_TEXTS]).is_ok());
}

#[test]
fn a_block_over_the_text_ceiling_is_refused_and_the_ceiling_is_exact() {
    let big = "a".repeat(MAX_BLOCK_TEXT_BYTES);
    assert!(encode_block(&[&big]).is_ok());
    let over = "a".repeat(MAX_BLOCK_TEXT_BYTES + 1);
    assert_eq!(
        encode_block(&[&over]).unwrap_err().kind(),
        FailureKind::Limit
    );
    assert_eq!(
        encode_block(&[&big, "b"]).unwrap_err().kind(),
        FailureKind::Limit
    );
}

// docs/specs/extension-analyzer AC-9: a split never goes inside a text
#[test]
fn texts_are_split_by_count_and_by_bytes_and_never_inside_a_text() {
    let texts = vec!["x"; 130];
    let refs: Vec<&str> = texts.clone();
    let r = split_blocks(&refs).expect("splits");
    assert_eq!(r, vec![0..64, 64..128, 128..130]);

    let half = "a".repeat(MAX_BLOCK_TEXT_BYTES / 2 + 1);
    let r = split_blocks(&[&half, &half, "z"]).expect("splits");
    assert_eq!(r, vec![0..1, 1..3]);

    let over = "a".repeat(MAX_BLOCK_TEXT_BYTES + 1);
    assert_eq!(
        split_blocks(&["a", &over]).unwrap_err().kind(),
        FailureKind::Limit
    );
    assert_eq!(
        split_blocks(&[]).expect("none"),
        Vec::<std::ops::Range<usize>>::new()
    );
}

// docs/specs/extension-analyzer AC-2
#[test]
fn a_valid_output_decodes_with_offsets_in_utf16_units() {
    // "a😀b": bytes a=0..1, 😀=1..5 (two UTF-16 units), b=5..6.
    let text = "a\u{1F600}b";
    let got = one(
        &[text],
        &[(b"a", 0, 1, 1), ("😀".as_bytes(), 1, 5, 1), (b"b", 5, 6, 2)],
    )
    .expect("decodes");
    let t = &got[0];
    assert_eq!(
        t[0],
        Token {
            term: "a".into(),
            start: 0,
            end: 1,
            increment: 1
        }
    );
    assert_eq!(
        t[1],
        Token {
            term: "😀".into(),
            start: 1,
            end: 3,
            increment: 1
        }
    );
    assert_eq!(
        t[2],
        Token {
            term: "b".into(),
            start: 3,
            end: 4,
            increment: 2
        }
    );
}

#[test]
fn a_text_with_no_tokens_and_two_texts_decode() {
    let mut out = Vec::new();
    tokens(&mut out, &[]);
    tokens(&mut out, &[(b"x", 0, 1, 1)]);
    let got = decode_block(&["  ", "x"], &out).expect("decodes");
    assert_eq!(got[0], vec![]);
    assert_eq!(got[1].len(), 1);
}

#[test]
fn a_term_that_is_empty_not_utf8_or_too_long_is_refused() {
    assert!(malformed(one(&["a"], &[(b"", 0, 1, 1)])).contains("empty term"));
    assert!(malformed(one(&["a"], &[(&[0xff, 0xfe], 0, 1, 1)])).contains("UTF-8"));
    let long = vec![b'a'; MAX_TERM_BYTES + 1];
    assert!(malformed(one(&["a"], &[(&long, 0, 1, 1)])).contains("term over"));
    let exact = vec![b'a'; MAX_TERM_BYTES];
    assert!(one(&["a"], &[(&exact, 0, 1, 1)]).is_ok());
}

#[test]
fn offsets_that_are_out_of_order_or_out_of_the_text_are_refused() {
    assert!(malformed(one(&["abc"], &[(b"a", 2, 1, 1)])).contains("start after end"));
    assert!(malformed(one(&["abc"], &[(b"a", 0, 4, 1)])).contains("past the text"));
    assert!(malformed(one(&["é"], &[(b"a", 0, 1, 1)])).contains("inside a character"));
    assert!(malformed(one(&["abc"], &[(b"a", 2, 3, 1), (b"b", 1, 2, 1)])).contains("decreases"));
}

#[test]
fn a_zero_first_increment_and_a_position_past_the_maximum_are_refused() {
    assert!(malformed(one(&["abc"], &[(b"a", 0, 1, 0)])).contains("first increment"));
    // A later zero is a stacked token and is fine.
    assert!(one(&["abc"], &[(b"a", 0, 1, 1), (b"b", 0, 1, 0)]).is_ok());
    let max = 0x8000_0000_u32;
    assert!(malformed(one(&["abc"], &[(b"a", 0, 1, max)])).contains("position"));
    assert!(
        malformed(one(&["abc"], &[(b"a", 0, 1, 1), (b"b", 1, 2, max - 1)])).contains("position")
    );
    // Position starts at -1: the last allowed first increment is 0x7FFFFFFF.
    assert!(one(&["abc"], &[(b"a", 0, 1, 0x7FFF_FFFF)]).is_ok());
}

#[test]
fn counts_that_do_not_fit_and_trailing_bytes_are_refused() {
    let mut out = Vec::new();
    word(&mut out, 5);
    assert!(malformed(decode_block(&["a"], &out)).contains("truncated"));
    let mut out = Vec::new();
    tokens(&mut out, &[(b"a", 0, 1, 1)]);
    out.push(0);
    assert!(malformed(decode_block(&["a"], &out)).contains("trailing"));
    // Too few texts in the output.
    let mut out = Vec::new();
    tokens(&mut out, &[]);
    assert!(malformed(decode_block(&["a", "b"], &out)).contains("truncated"));
}

#[test]
fn more_tokens_than_the_ceiling_is_a_limit_and_an_oversized_output_is_malformed() {
    let mut out = Vec::new();
    word(&mut out, MAX_TOKENS as u32 + 1);
    let e = decode_block(&["a"], &out).expect_err("limit");
    assert_eq!((e.kind(), e.http_status()), (FailureKind::Limit, 400));
    let big = vec![0_u8; MAX_OUTPUT_BYTES + 1];
    assert!(malformed(decode_block(&["a"], &big)).contains("output over"));
}
