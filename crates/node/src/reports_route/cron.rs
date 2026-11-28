//! The shape of a five-field cron expression: minute, hour, day of month,
//! month, day of week.

/// `(low, high)` of each field.
const RANGES: [(u32, u32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];

fn number(s: &str, (low, high): (u32, u32)) -> bool {
    s.parse::<u32>().is_ok_and(|n| (low..=high).contains(&n))
}

/// One comma-separated item: `*`, `n`, `a-b`, each with an optional `/step`.
fn item(s: &str, range: (u32, u32)) -> bool {
    let (base, step) = match s.split_once('/') {
        Some((b, st)) => (b, Some(st)),
        None => (s, None),
    };
    if step.is_some_and(|st| st.parse::<u32>().map_or(true, |n| n == 0)) {
        return false;
    }
    if base == "*" {
        return true;
    }
    match base.split_once('-') {
        Some((a, b)) => number(a, range) && number(b, range),
        None => number(base, range),
    }
}

/// Whether `expression` has five fields, each of valid items for its range.
pub(super) fn valid(expression: &str) -> bool {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    fields.len() == RANGES.len()
        && fields
            .iter()
            .zip(RANGES)
            .all(|(f, r)| f.split(',').all(|i| item(i, r)))
}

#[cfg(test)]
mod tests {
    use super::valid;

    #[test]
    fn five_fields_of_valid_items_are_a_cron_and_nothing_else_is() {
        for ok in ["* * * * *", "0 9 * * 1-5", "*/15 0,12 1 1 *", "30 6 * * 7"] {
            assert!(valid(ok), "{ok}");
        }
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "*/0 * * * *",
            "a * * * *",
            "5- * * * *",
        ] {
            assert!(!valid(bad), "{bad}");
        }
    }
}
