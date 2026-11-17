//! The order a response lists aggregations in: `InternalAggregations.reduce`
//! groups them by name in a `java.util.HashMap` and lists them in its
//! iteration order, which a single-shard search answers in too
//! (server/src/main/java/org/opensearch/search/aggregations/InternalAggregations.java,
//! at OpenSearch 3.10.0).
//!
//! A `HashMap` lists its table's buckets in index order and each bucket's
//! entries in the order they were put. A key's bucket is its spread
//! `String.hashCode` (`h ^ (h >>> 16)`) masked by the table's size, which
//! starts at 16 and doubles once the map holds more than three quarters
//! of it, or when a bucket would grow past 8 entries in a table under 64.
//! A bucket of more than 8 in a table of 64 or more becomes a tree, which
//! moves its root first; that takes at least 9 names sharing their low 6
//! hash bits and is not modelled.

/// `String.hashCode`: over UTF-16 units, `h = 31 * h + c`, wrapping.
pub fn java_string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}

/// `HashMap.hash`: the hash's high half folded into its low half.
fn spread(s: &str) -> u32 {
    let h = java_string_hash(s).cast_unsigned();
    h ^ (h >> 16)
}

/// The indexes of `names` (distinct, in the order put) in the order a
/// `HashMap` keyed by them iterates.
pub fn hash_map_order(names: &[&str]) -> Vec<usize> {
    let mut capacity = 16usize;
    // ALLOC: one entry a name, bounded by the request's aggregations.
    let hashes: Vec<u32> = names.iter().map(|n| spread(n)).collect();
    let bucket_of = |i: usize, capacity: usize| -> usize {
        let mask = u32::try_from(capacity.saturating_sub(1)).unwrap_or(u32::MAX);
        usize::try_from(hashes.get(i).copied().unwrap_or(0) & mask).unwrap_or(0)
    };
    for size in 0..names.len() {
        // `putVal`: a ninth entry in one bucket resizes a table under 64.
        let bucket = bucket_of(size, capacity);
        let sharing = (0..size)
            .filter(|&j| bucket_of(j, capacity) == bucket)
            .count();
        if sharing >= 8 && capacity < 64 {
            capacity = capacity.saturating_mul(2);
        }
        // Then a map past three quarters full doubles.
        let threshold = (capacity / 4).saturating_mul(3);
        if size.saturating_add(1) > threshold {
            capacity = capacity.saturating_mul(2);
        }
    }
    // ALLOC: one entry a name, as above.
    let mut order: Vec<usize> = (0..names.len()).collect();
    // A stable sort keeps each bucket's entries in the order they were put.
    order.sort_by_key(|&i| bucket_of(i, capacity));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_java_string_hash_codes() {
        // `"...".hashCode()` in Java.
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("a"), 97);
        assert_eq!(java_string_hash("min_year"), -1_367_949_814);
        assert_eq!(java_string_hash("polygenelubricants"), i32::MIN);
        assert_eq!(java_string_hash("é"), 233);
    }

    #[test]
    fn names_are_listed_as_a_hash_map_iterates_them() {
        // `new HashMap<>()` put in this order lists "b", "a", "c" ... as
        // Java does: buckets 1, 2, 3 for "a", "b", "c" (97, 98, 99 & 15).
        assert_eq!(hash_map_order(&["c", "b", "a"]), [2, 1, 0]);
        assert_eq!(hash_map_order(&[]), Vec::<usize>::new());
        // "q" (113 & 15 = 1) shares "a"'s bucket and stays after it.
        assert_eq!(hash_map_order(&["q", "a", "b"]), [0, 1, 2]);
        assert_eq!(hash_map_order(&["b", "q", "a"]), [1, 2, 0]);
    }

    #[test]
    fn a_thirteenth_name_doubles_the_table() {
        // 13 single letters fill more than 12 of 16 slots: the table is 32,
        // so "a" (97 & 31 = 1) comes before "q" (113 & 31 = 17) whatever
        // the order they were put in.
        let names = [
            "q", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
        ];
        let order = hash_map_order(&names);
        let pos = |name: &str| {
            order
                .iter()
                .position(|&i| names[i] == name)
                .expect("listed")
        };
        assert!(pos("a") < pos("q"));
        assert_eq!(order.len(), 13);
    }
}
