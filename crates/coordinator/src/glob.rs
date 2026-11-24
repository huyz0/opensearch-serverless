//! The simple `*` pattern index names are matched with.

/// Whether `pattern` matches all of `name`, `*` standing for any run of
/// characters and every other character for itself.
pub(crate) fn glob(pattern: &str, name: &str) -> bool {
    // ALLOC: the two names as characters, bounded by an index name and a pattern.
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi = pi.saturating_add(1);
            }
            Some(c) if Some(c) == n.get(ni) => {
                pi = pi.saturating_add(1);
                ni = ni.saturating_add(1);
            }
            _ => match star {
                Some((sp, sn)) => {
                    pi = sp.saturating_add(1);
                    ni = sn.saturating_add(1);
                    star = Some((sp, sn.saturating_add(1)));
                }
                None => return false,
            },
        }
    }
    p.get(pi..)
        .is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}
