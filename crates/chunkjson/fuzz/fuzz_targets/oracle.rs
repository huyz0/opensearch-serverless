//! Arbitrary bytes, split at a size taken from the first byte: the tokenizer
//! accepts exactly what serde_json accepts, and rebuilds the same value, except
//! that a number longer than `max_number_len` is refused.
#![no_main]

use chunkjson::{Config, ErrorKind, Event, Tokenizer};
use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value};

/// `Err` is a refusal by `max_number_len`, which serde_json has no equivalent of.
fn parse(bytes: &[u8], size: usize) -> Result<Option<Value>, ()> {
    let mut tok = Tokenizer::new(Config::default());
    let mut stack: Vec<(Value, Option<String>)> = Vec::new();
    let (mut text, mut out) = (String::new(), None);
    let mut put = |stack: &mut Vec<(Value, Option<String>)>, v: Value, out: &mut Option<Value>| match stack.last_mut() {
        None => *out = Some(v),
        Some((Value::Array(a), _)) => a.push(v),
        Some((Value::Object(m), k)) => {
            m.insert(k.take().unwrap(), v);
        }
        Some(_) => unreachable!(),
    };
    for (n, chunk) in bytes.chunks(size).chain(std::iter::once(&[][..])).enumerate() {
        let eof = n * size >= bytes.len();
        let mut chunk = chunk;
        loop {
            let (event, used) = match tok.next(chunk, eof) {
                Ok(r) => r,
                Err(e) if e.kind() == ErrorKind::NumberTooLong => return Err(()),
                Err(_) => return Ok(None),
            };
            chunk = &chunk[used..];
            match event {
                Event::BeginObject => stack.push((Value::Object(Map::new()), None)),
                Event::BeginArray => stack.push((Value::Array(Vec::new()), None)),
                Event::EndObject | Event::EndArray => {
                    let (v, _) = stack.pop().unwrap();
                    put(&mut stack, v, &mut out);
                }
                Event::Key(t) | Event::String(t) => {
                    text.push_str(t.fragment);
                    if t.last {
                        let s = std::mem::take(&mut text);
                        if matches!(event, Event::Key(_)) {
                            stack.last_mut().unwrap().1 = Some(s);
                        } else {
                            put(&mut stack, Value::String(s), &mut out);
                        }
                    }
                }
                Event::Number(t) => match serde_json::from_str(t) {
                    Ok(v) => put(&mut stack, v, &mut out),
                    Err(_) => return Ok(None),
                },
                Event::Bool(b) => put(&mut stack, Value::Bool(b), &mut out),
                Event::Null => put(&mut stack, Value::Null, &mut out),
                Event::EndDocument { .. } => {}
                Event::NeedMore => break,
                Event::Eof => return Ok(out),
            }
        }
    }
    Ok(None)
}

fuzz_target!(|data: &[u8]| {
    let Some((&first, bytes)) = data.split_first() else { return };
    let size = usize::from(first % 16) + 1;
    let oracle = serde_json::from_slice::<Value>(bytes);
    if matches!(&oracle, Err(e) if e.to_string().contains("number out of range") || e.to_string().contains("recursion limit")) {
        return;
    }
    match parse(bytes, size) {
        Ok(got) => assert_eq!(got, oracle.ok(), "input {:?}", String::from_utf8_lossy(bytes)),
        // The refusal is right only if some run of number bytes really exceeds the limit.
        Err(()) => {
            let limit = Config::default().max_number_len;
            let longest = bytes
                .split(|b| !matches!(b, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'))
                .map(<[u8]>::len)
                .max()
                .unwrap_or(0);
            assert!(longest > limit, "NumberTooLong on a {longest}-byte run");
        }
    }
});
