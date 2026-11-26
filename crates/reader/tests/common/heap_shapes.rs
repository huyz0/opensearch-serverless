//! Record and mapping shapes the overlay heap tests measure.

// `wide` is used by one of the two binaries that include this file.
// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

use oss_doc::mapping::Mapping;
use oss_format::WalRecord;

pub fn mapping() -> Mapping {
    Mapping::parse(
        br#"{"properties":{"name":{"type":"keyword"},"body":{"type":"text"},"n":{"type":"long"},
        "a":{"type":"keyword"},"b":{"type":"keyword"},"c":{"type":"text"},"d":{"type":"long"}}}"#,
    )
    .expect("mapping")
}

/// `count` records whose `body` is `body_bytes` long, with every field set.
pub fn records(count: usize, body_bytes: usize) -> Vec<WalRecord<'static>> {
    let words = ["alpha", "beta", "gamma", "delta", "kappa", "omega", "sigma"];
    (0..count)
        .map(|i| {
            let mut body = String::new();
            let mut w = i;
            while body.len() < body_bytes {
                body.push_str(words[w % words.len()]);
                body.push(' ');
                w += 3;
            }
            body.truncate(body_bytes);
            let source = format!(
                r#"{{"name":"n{i}","body":"{body}","n":{i},"a":"a{}","b":"b{}","c":"c {i} text","d":{}}}"#,
                i % 7,
                i % 13,
                i * 3
            );
            WalRecord::index(
                format!("id{i}"),
                source,
                i64::try_from(i).expect("fits") + 1,
                1,
                1,
            )
        })
        .collect()
}

/// `count` records of `fields` keyword fields each, every value `value_bytes`
/// of a pseudo-random run that does not compress.
#[allow(dead_code)]
pub fn wide(count: usize, fields: usize, value_bytes: usize) -> (Mapping, Vec<WalRecord<'static>>) {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let props: Vec<String> = (0..fields)
        .map(|f| format!(r#""f{f}":{{"type":"keyword"}}"#))
        .collect();
    let mapping = Mapping::parse(format!(r#"{{"properties":{{{}}}}}"#, props.join(",")).as_bytes())
        .expect("mapping");
    let records = (0..count)
        .map(|i| {
            let values: Vec<String> = (0..fields)
                .map(|f| {
                    let text: String = (0..value_bytes)
                        .map(|_| char::from(b'a' + u8::try_from(next() % 26).expect("small")))
                        .collect();
                    format!(r#""f{f}":"{text}""#)
                })
                .collect();
            WalRecord::index(
                format!("id{i}"),
                format!("{{{}}}", values.join(",")),
                i64::try_from(i).expect("fits") + 1,
                1,
                1,
            )
        })
        .collect();
    (mapping, records)
}
