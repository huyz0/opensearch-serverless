//! The tokens of a module's analyzer in a field's postings.

use lucene_codecs::field_infos::IndexOptions;
use oss_ext::analyzer::Token;

use super::Inverted;

fn token(term: &str, start: u32, end: u32, increment: u32) -> Token {
    Token {
        term: term.into(),
        start,
        end,
        increment,
    }
}

fn field(values: &[(&[Token], i32, i32)]) -> lucene_index::index_writer::InvertedField {
    let mut inverted = Inverted::new(7, IndexOptions::DocsAndFreqsAndPositionsAndOffsets, true);
    for (tokens, units, gap) in values {
        inverted.tokens(tokens, *units, *gap).expect("tokens fit");
    }
    inverted.finish().expect("a field with terms")
}

#[test]
fn a_values_tokens_take_their_positions_from_minus_one() {
    let f = field(&[(&[token("a", 0, 1, 1), token("b", 2, 3, 1)], 3, 100)]);
    let at = |t: &[u8]| f.terms.iter().find(|x| x.term == t).expect("term");
    assert_eq!(at(b"a").positions, [0]);
    assert_eq!(at(b"b").positions, [1]);
    assert_eq!(at(b"b").offsets, [(2, 3)]);
}

#[test]
fn a_second_value_sits_behind_the_gap_and_an_offset_gap_of_one() {
    let first = [token("a", 0, 1, 1)];
    let second = [token("b", 0, 1, 1)];
    let f = field(&[(&first, 1, 100), (&second, 1, 100)]);
    let at = |t: &[u8]| f.terms.iter().find(|x| x.term == t).expect("term");
    assert_eq!(at(b"a").positions, [0]);
    assert_eq!(at(b"b").positions, [101]);
    // The first value is one unit long, the offset gap adds 1.
    assert_eq!(at(b"b").offsets, [(2, 3)]);
}

#[test]
fn a_token_at_the_same_position_is_an_overlap_left_out_of_the_norm() {
    let overlapping = field(&[(&[token("a", 0, 1, 1), token("b", 0, 1, 0)], 1, 0)]);
    let plain = field(&[(&[token("a", 0, 1, 1)], 1, 0)]);
    assert_eq!(overlapping.norm, plain.norm);
}
