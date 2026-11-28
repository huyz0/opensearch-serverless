//! The order the plugin lists the columns of `SELECT *`: a Java `HashMap` over the
//! top-level field names, which it filled in name order.

/// Java's `String.hashCode`.
fn java_hash(s: &str) -> u32 {
    s.encode_utf16()
        .fold(0_u32, |h, c| h.wrapping_mul(31).wrapping_add(u32::from(c)))
}

/// `names` (already in name order) in the order a Java `HashMap` iterates them.
pub(crate) fn iteration_order(names: Vec<String>) -> Vec<String> {
    let mut table: usize = 16;
    while names.len() > table.saturating_mul(3) / 4 {
        table = table.saturating_mul(2);
    }
    let mut keyed: Vec<(usize, usize, String)> = names
        .into_iter()
        .enumerate()
        .map(|(i, n)| {
            let h = java_hash(&n);
            let spread = h ^ (h >> 16);
            // The table is a power of two, so the mask is the low bits of the spread hash.
            let bucket = usize::try_from(spread).unwrap_or(0) & table.saturating_sub(1);
            (bucket, i, n)
        })
        .collect();
    keyed.sort();
    keyed.into_iter().map(|(_, _, n)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::iteration_order;

    #[test]
    fn eleven_names_come_out_in_the_recorded_order() {
        let mut names: Vec<String> = [
            "name", "age", "city", "bio", "balance", "active", "joined", "ip", "score", "address",
            "tags",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        names.sort();
        assert_eq!(
            iteration_order(names),
            [
                "score", "address", "balance", "city", "joined", "ip", "name", "active", "bio",
                "age", "tags"
            ]
        );
    }

    #[test]
    fn a_table_that_grows_buckets_by_the_larger_mask() {
        let names: Vec<String> = (0..13).map(|i| format!("f{i:02}")).collect();
        let out = iteration_order(names.clone());
        assert_eq!(out.len(), 13);
        let mut sorted = out.clone();
        sorted.sort();
        assert_eq!(sorted, names);
    }
}
