# Verified: a searcher over the commit and the overlay

- AC-1: `the_overlay_replaces_and_deletes_commit_documents` in crates/reader/tests/overlay_view.rs
- AC-2: `with_no_overlay_the_commit_answers_as_before` in crates/reader/tests/overlay_view.rs
- AC-3: `a_mask_keeps_the_commits_own_deletions` in crates/reader/tests/overlay_view.rs
- AC-4: `a_scored_text_query_reads_both_sides` in crates/reader/tests/overlay_view.rs
- AC-5: `overlay_documents_are_numbered_after_the_commits` in crates/reader/tests/overlay_view.rs
- AC-6: `masks_ignore_what_the_commit_does_not_hold_and_an_empty_overlay_has_no_norms` in crates/reader/tests/overlay_view.rs
