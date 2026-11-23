//! The order OpenSearch writes the maps of a `_rank_eval` response in: the
//! `HashMap` of the details and the `ConcurrentHashMap` of the failures,
//! each sized for the number of requests.

/// `String.hashCode`.
fn java_hash(s: &str) -> u32 {
    s.encode_utf16()
        .fold(0_u32, |h, c| h.wrapping_mul(31).wrapping_add(u32::from(c)))
}

/// `Integer.highestOneBit`-based `tableSizeFor`: the next power of two.
fn table_size_for(n: usize) -> usize {
    n.max(1).next_power_of_two()
}

/// Positions of `keys` (distinct, in the order put) in the iteration order
/// of `new HashMap<>(capacity)` after they were put.
pub fn hash_map_order(keys: &[&str], capacity: usize) -> Vec<usize> {
    let mut cap = table_size_for(capacity);
    let mut threshold = cap.saturating_mul(3).saturating_div(4);
    for size in 1..=keys.len() {
        if size > threshold {
            cap = cap.saturating_mul(2);
            threshold = cap.saturating_mul(3).saturating_div(4);
        }
    }
    order_by_bucket(keys, cap, false)
}

/// As [`hash_map_order`] for `new ConcurrentHashMap<>(capacity)`.
pub fn concurrent_map_order(keys: &[&str], capacity: usize) -> Vec<usize> {
    // The constructor sizes the table for `1 + capacity / 0.75`.
    let mut cap = table_size_for(
        capacity
            .saturating_mul(4)
            .saturating_div(3)
            .saturating_add(1),
    );
    for size in 1..=keys.len() {
        if size >= cap.saturating_sub(cap >> 2) {
            cap = cap.saturating_mul(2);
        }
    }
    order_by_bucket(keys, cap, true)
}

fn order_by_bucket(keys: &[&str], cap: usize, concurrent: bool) -> Vec<usize> {
    let bucket = |k: &str| {
        let h = java_hash(k);
        let mut spread = h ^ (h >> 16);
        if concurrent {
            spread &= 0x7fff_ffff;
        }
        usize::try_from(spread).unwrap_or(0) & cap.saturating_sub(1)
    };
    // ALLOC: one entry a key, which the request bounds.
    let mut order: Vec<usize> = (0..keys.len()).collect();
    order.sort_by_key(|&i| keys.get(i).map_or(0, |k| bucket(k)));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(keys: &[&str], order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| keys[i].to_owned()).collect()
    }

    #[test]
    fn the_orders_match_the_java_maps() {
        let fixtures: fixture::Fixtures = fixture::load();
        for case in fixtures.cases {
            let keys: Vec<&str> = case.keys.iter().map(String::as_str).collect();
            assert_eq!(
                names(&keys, &hash_map_order(&keys, case.capacity)),
                case.hash_map,
                "HashMap({}) {:?}",
                case.capacity,
                case.keys
            );
            assert_eq!(
                names(&keys, &concurrent_map_order(&keys, case.capacity)),
                case.concurrent,
                "ConcurrentHashMap({}) {:?}",
                case.capacity,
                case.keys
            );
        }
    }

    /// A reader of the recorded fixture, with no JSON dependency: one case a
    /// line, `capacity|keys|hashmap order|concurrent order`, lists joined by
    /// commas.
    mod fixture {
        pub struct Case {
            pub capacity: usize,
            pub keys: Vec<String>,
            pub hash_map: Vec<String>,
            pub concurrent: Vec<String>,
        }
        pub struct Fixtures {
            pub cases: Vec<Case>,
        }
        pub fn load() -> Fixtures {
            let text = include_str!("../../../../fixtures/rank_eval/map_order.txt");
            let list = |s: &str| s.split(',').map(str::to_owned).collect::<Vec<_>>();
            Fixtures {
                cases: text
                    .lines()
                    .map(|l| {
                        let f: Vec<&str> = l.split('|').collect();
                        Case {
                            capacity: f[0].parse().expect("capacity"),
                            keys: list(f[1]),
                            hash_map: list(f[2]),
                            concurrent: list(f[3]),
                        }
                    })
                    .collect(),
            }
        }
    }
}
