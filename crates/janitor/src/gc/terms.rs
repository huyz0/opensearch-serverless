//! Which blobs of a shard's path a sweep judges at all.

/// Whether `key`, under the shard's path `shard_path`, is a blob in a term
/// container strictly older than `live`: `t=<digits>/<name>` with a term
/// below `live` and a name that is one key part. The manifest register, a
/// container that is not a term and a nested key are never judged.
pub(super) fn is_older_term_blob(key: &str, shard_path: &str, live: u64) -> bool {
    let Some(rest) = key.strip_prefix(shard_path) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("t=") else {
        return false;
    };
    let Some((term, name)) = rest.split_once('/') else {
        return false;
    };
    if name.is_empty() || name.contains('/') {
        return false;
    }
    if term.is_empty() || !term.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    term.parse::<u64>().is_ok_and(|term| term < live)
}

#[cfg(test)]
mod tests {
    use super::is_older_term_blob;

    const P: &str = "d/segments/i#u#0/";

    #[test]
    fn only_a_blob_of_a_strictly_older_term_is_judged() {
        assert!(is_older_term_blob("d/segments/i#u#0/t=1/_0.seg", P, 2));
        assert!(!is_older_term_blob("d/segments/i#u#0/t=2/_0.seg", P, 2));
        assert!(!is_older_term_blob("d/segments/i#u#0/t=3/_0.seg", P, 2));
        assert!(is_older_term_blob("d/segments/i#u#0/t=9/_0.seg", P, 10));
    }

    #[test]
    fn what_is_not_a_term_blob_is_not_judged() {
        for key in [
            "d/segments/i#u#0/manifest",
            "d/segments/i#u#0/t=1",
            "d/segments/i#u#0/t=1/",
            "d/segments/i#u#0/t=x/_0.seg",
            "d/segments/i#u#0/t=+1/_0.seg",
            "d/segments/i#u#0/t=/_0.seg",
            "d/segments/i#u#0/t=1/a/b",
            "d/segments/i#u#0/u=1/_0.seg",
            "d/segments/i#u#1/t=1/_0.seg",
            "d/other/t=1/_0.seg",
            "d/segments/i#u#0/t=99999999999999999999999/_0.seg",
        ] {
            assert!(!is_older_term_blob(key, P, u64::MAX), "{key}");
        }
    }
}
