# The coordinator merges the hits of every shard into one page

## Serves

Parity plan R7.2, search fan-out and reduce, first piece: the hit half of the Java shell's `SearchMerge` (ops package, shell 429b4cbc): totals, relation, best score, flags, the ordered, windowed merge of hits and the page cut from it. The aggregation reduce, the memory charge and the fan-out itself are their own pieces.

## Change

`oss-coordinator` gains `reduce::hits`: a `Merger` that takes each shard's answer (`total`, whether exact, best score, hits with their score and sort values and a caller's payload, `timed_out`, `terminated_early`) and gives the `Merged` result: the total reported for `track_total_hits_up_to`, its relation, the best score, the flags, how many shards answered, and the page of hits at `from` and `size`.

## Acceptance criteria

- AC-1: with no sort, hits order by score descending; equal scores keep the order the shards answered in; a hit with no score sorts last.
- AC-2: with a field sort, each key runs in the direction the request gave; a missing value (`Term(None)`) goes last unless the sort says `_first`, whichever the direction; a hit with no sort values goes after every hit with them.
- AC-3: sort values of different kinds at one key are refused with "cannot merge incompatible sort values across indices; use consistent field mappings or numeric_type for numeric sorts" when they are compared.
- AC-4: totals add; the relation is "at least" once any shard's is; the best score is the highest any shard had; `timed_out` is any shard's; `terminated_early` is unset until a shard sets it, then any shard's true; `answered` counts the answers.
- AC-5: the pile never holds more than twice the window (`from + size`, at least 1) plus the answer being added: past that it is sorted and cut back to the window, and the page is the same as without the cut.
- AC-6: the page is the hits from `from`, at most `size`, of the sorted pile.
- AC-7: a total that is only "at least" is reported as at most `track_total_hits_up_to` (the default 10,000 when unset); a bound of zero or below, or the accurate bound (`i32::MAX`), reports it as counted; an exact total is reported as counted.

## Test plan

`crates/coordinator/tests/it/hit_merge.rs`: one test for each criterion over hand-built answers, the ordering and the window asserted exactly.

## S3 request budget

Zero: the merge reads nothing.

## Bounds

At most twice the window of hits at once plus one shard's answer (each shard sends at most the window).

## Switch

None: the shell's merge has no setting and no measured downside.

## Risks

The memory charge of the pile (the shell's breaker) belongs to the piece that holds the node's budget.
