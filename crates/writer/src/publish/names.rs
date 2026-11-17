//! Which segment a Lucene file belongs to.

/// The segment a file name belongs to (`IndexFileNames.parseSegmentName`):
/// `_0` for `_0.cfs`, `_0_1.liv` and `_0_Lucene90_0.dvd`; a name with
/// neither a second `_` nor a `.` is its own, as `segments_5` is.
pub fn segment_name(file: &str) -> &str {
    let end = file
        .get(1..)
        .and_then(|rest| rest.find('_'))
        .map(|i| i.saturating_add(1))
        .or_else(|| file.find('.'));
    match end {
        Some(i) => file.get(..i).unwrap_or(file),
        None => file,
    }
}

#[cfg(test)]
mod tests {
    use super::segment_name;

    #[test]
    fn a_files_segment_is_what_comes_before_its_generation_or_extension() {
        for (file, segment) in [
            ("_0.cfs", "_0"),
            ("_12.si", "_12"),
            ("_0_1.liv", "_0"),
            ("_a_Lucene90_0.dvd", "_a"),
            ("_0.fnm", "_0"),
            ("segments_5", "segments"),
            ("write.lock", "write"),
            ("plain", "plain"),
        ] {
            assert_eq!(segment_name(file), segment, "{file}");
        }
    }
}
