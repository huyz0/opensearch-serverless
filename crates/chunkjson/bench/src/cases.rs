//! Generated inputs: deterministic, so every contender sees the same bytes.

pub const NAMES: [&str; 4] = ["bulk", "records", "numbers", "big_text"];

/// A tiny deterministic generator (xorshift), so no dependency is needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const WORDS: [&str; 16] = [
    "page", "confluence", "search", "index", "shard", "reader", "writer", "segment", "naïve", "café", "größe", "日本語",
    "budget", "stream", "the", "of",
];

fn words(rng: &mut Rng, out: &mut String, n: usize) {
    for i in 0..n {
        if i > 0 {
            out.push(if rng.below(40) == 0 { '\n' } else { ' ' });
        }
        out.push_str(WORDS[rng.below(16) as usize]);
        if rng.below(60) == 0 {
            out.push('"');
        }
    }
}

fn doc(rng: &mut Rng, i: u64, body_words: usize) -> serde_json::Value {
    let mut title = String::new();
    words(rng, &mut title, 6);
    let mut body = String::new();
    words(rng, &mut body, body_words);
    let tags: Vec<&str> = (0..rng.below(5)).map(|_| WORDS[rng.below(16) as usize]).collect();
    serde_json::json!({
        "id": i,
        "title": title,
        "body": body,
        "tags": tags,
        "score": rng.below(1_000_000) as f64 / 1000.0,
        "published": rng.below(2) == 0,
        "author": {"name": "someone", "id": rng.below(10_000), "groups": [1, 2, 3]},
        "parent": null,
    })
}

pub fn build(case: &str) -> Vec<u8> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::new();
    match case {
        // _bulk: an action line and a ~1 KB source line per document.
        "bulk" => {
            for i in 0..20_000 {
                out.extend_from_slice(format!("{{\"index\":{{\"_index\":\"pages\",\"_id\":\"{i}\"}}}}\n").as_bytes());
                serde_json::to_writer(&mut out, &doc(&mut rng, i, 120)).unwrap();
                out.push(b'\n');
            }
        }
        // One array of many small objects: key- and structure-heavy.
        "records" => {
            let docs: Vec<_> = (0..40_000).map(|i| doc(&mut rng, i, 8)).collect();
            serde_json::to_writer(&mut out, &docs).unwrap();
        }
        // Coordinates: number-heavy, as in the canada.json corpus.
        "numbers" => {
            let pts: Vec<[f64; 2]> =
                (0..500_000).map(|_| [rng.below(36_000_000) as f64 / 1e5 - 180.0, rng.below(18_000_000) as f64 / 1e5 - 90.0]).collect();
            serde_json::to_writer(&mut out, &serde_json::json!({"type": "Polygon", "coordinates": [pts]})).unwrap();
        }
        // One huge document: extracted PDF or Confluence text, ~128 MB in one string.
        "big_text" => {
            let mut text = String::with_capacity(130 << 20);
            while text.len() < 128 << 20 {
                words(&mut rng, &mut text, 1000);
            }
            serde_json::to_writer(&mut out, &serde_json::json!({"title": "big", "content": text, "pages": 4000})).unwrap();
        }
        _ => panic!("unknown case {case}"),
    }
    out
}

pub fn is_ndjson(case: &str) -> bool {
    case == "bulk"
}
