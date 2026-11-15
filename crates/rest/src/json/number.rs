//! The JSON number grammar: `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`.

/// Whether `b` can continue a number token.
pub(super) fn continues(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')
}

/// Whether `s` is a complete JSON number.
pub(super) fn is_valid(s: &[u8]) -> bool {
    let mut i = 0;
    let at = |i: usize| s.get(i).copied();
    if at(i) == Some(b'-') {
        i = 1;
    }
    match at(i) {
        Some(b'0') => i = i.saturating_add(1),
        Some(b'1'..=b'9') => i = digits(s, i),
        _ => return false,
    }
    if at(i) == Some(b'.') {
        let after = digits(s, i.saturating_add(1));
        if after == i.saturating_add(1) {
            return false;
        }
        i = after;
    }
    if matches!(at(i), Some(b'e' | b'E')) {
        i = i.saturating_add(1);
        if matches!(at(i), Some(b'+' | b'-')) {
            i = i.saturating_add(1);
        }
        let after = digits(s, i);
        if after == i {
            return false;
        }
        i = after;
    }
    i == s.len()
}

/// The index after the run of digits starting at `i`.
fn digits(s: &[u8], i: usize) -> usize {
    let n = s
        .get(i..)
        .unwrap_or_default()
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    i.saturating_add(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_and_invalid_numbers() {
        for ok in [
            "0", "-0", "1", "42", "-17", "0.5", "1.25e10", "1E+2", "1e-2", "-0.0e0",
        ] {
            assert!(is_valid(ok.as_bytes()), "{ok}");
        }
        for bad in [
            "", "-", "01", "1.", ".5", "1e", "1e+", "+1", "1.2.3", "--1", "1-", "0x1", "1ee2",
        ] {
            assert!(!is_valid(bad.as_bytes()), "{bad}");
        }
        assert!(continues(b'e') && continues(b'7') && !continues(b','));
    }
}
