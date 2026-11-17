//! A field value as the Java's error message previews it: the value read
//! into Java collections and printed with `toString`, members of an object
//! in `HashMap` iteration order.

use crate::mapping::json::{Json, is_integral, java_float, java_number};

/// `String.hashCode`, over UTF-16 code units.
fn java_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, u| {
        h.wrapping_mul(31)
            .wrapping_add(i32::from(u as i16) & 0xffff)
    })
}

/// `HashMap.hash`: the high half folded into the low half.
fn spread(s: &str) -> u32 {
    let h = java_hash(s) as u32;
    h ^ (h >> 16)
}

/// The order a `HashMap` built by putting `keys` in turn iterates them:
/// by bucket of the final table, then by insertion within a bucket.
pub fn hash_order(keys: &[&str]) -> Vec<usize> {
    // The table starts at 16 and doubles past three quarters full.
    let mut capacity: usize = 16;
    while keys.len() > (capacity / 4).saturating_mul(3) {
        capacity = capacity.saturating_mul(2);
    }
    let mask = (capacity as u32).wrapping_sub(1);
    // ALLOC: one index a member; error-message path.
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| (keys.get(i).map_or(0, |k| spread(k) & mask), i));
    order
}

/// The preview of `value`; `float_field` previews a number as the `Float`
/// a float field read it as.
pub fn preview(value: &Json, float_field: bool) -> String {
    // ALLOC: one message; error path.
    let mut out = String::new();
    match value {
        Json::Num(t) if float_field && !t.is_empty() => {
            let v: f64 = t.parse().unwrap_or(f64::NAN);
            #[allow(clippy::cast_possible_truncation)]
            out.push_str(&java_float(v as f32));
        }
        v => write(v, &mut out),
    }
    out
}

fn write(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(t) if is_integral(t) => out.push_str(t),
        Json::Num(t) => out.push_str(&java_number(t)),
        Json::Str(s) => out.push_str(s),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(item, out);
            }
            out.push(']');
        }
        Json::Obj(members) => {
            // ALLOC: one key reference a member; error path.
            let keys: Vec<&str> = members.iter().map(|(k, _)| k.as_str()).collect();
            out.push('{');
            for (n, i) in hash_order(&keys).into_iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                if let Some((k, v)) = members.get(i) {
                    out.push_str(k);
                    out.push('=');
                    write(v, out);
                }
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_map_order() {
        // `"b".hashCode()` is 98 and `"a"` 97: bucket order, not insertion.
        assert_eq!(hash_order(&["b", "a"]), vec![1, 0]);
        // "Aa" and "BB" share a hash, so they keep insertion order.
        assert_eq!(hash_order(&["BB", "Aa"]), vec![0, 1]);
        // The thirteenth put grows the table to 32, which moves "p" (112)
        // from bucket 0 to bucket 16, after "a" to "l" (97 to 108); with
        // twelve keys the table is still 16 and "p" comes first.
        let mut keys: Vec<&str> = vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"];
        keys.push("p");
        assert_eq!(hash_order(&keys).first(), Some(&11));
        keys.insert(11, "l");
        assert_eq!(hash_order(&keys), (0..13).collect::<Vec<_>>());
        let v = Json::Obj(vec![
            ("c".into(), Json::Str("x".into())),
            ("a".into(), Json::Num("1".into())),
            (
                "b".into(),
                Json::Arr(vec![Json::Num("1".into()), Json::Null, Json::Bool(true)]),
            ),
        ]);
        assert_eq!(preview(&v, false), "{a=1, b=[1, null, true], c=x}");
        assert_eq!(preview(&Json::Num("1e39".into()), true), "Infinity");
        assert_eq!(preview(&Json::Num("3e9".into()), false), "3.0E9");
        assert_eq!(
            preview(&Json::Num("12345678901234567890".into()), false),
            "12345678901234567890"
        );
    }
}
