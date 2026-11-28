# SQL, slice 4: SHOW TABLES and DESCRIBE over a pattern

## Serves

[ADR 0082](../../adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md), the metadata statements of order of work item 6. Builds on [sql-select](../sql-select/SPEC.md), whose AC-8 deferred `SHOW TABLES` and `DESCRIBE ... LIKE` to the name listing. The reference is the recorded corpus in `fixtures/sql`: `show-tables`, `show-tables-all`, `describe`, `describe-columns`.

## Change

`SHOW TABLES LIKE <pattern>` and `DESCRIBE TABLES LIKE <pattern> [COLUMNS LIKE <pattern>]` accept a SQL pattern (`%` any run, `_` one character) over index names.

The names come from the one derived listing the node already has for `_cat/indices`: the descriptor store's capped prefix listing (`names_with_prefix`). The planner takes the literal prefix of the pattern (the text before its first `%` or `_`), the node lists that prefix once, and `like` filters the names. Nothing else lists: a pattern with no wildcard names one index and lists nothing (as `DESCRIBE` of one name does today).

- One marked LIST per statement, whatever the pattern and however many indices match, up to the cap; the LIST is the only request a `SHOW TABLES` costs. A `DESCRIBE` adds the mapping read of each matched index, which is the open a search of it would make.
- A prefix with more than the cap of live names (1,000, the cap `_cat/indices` uses) is a 400 telling the caller to narrow the pattern. It is never cut: a cut listing answers a different question.
- A name the caller may not read is left out of the answer (the stock plugin shows what the caller can see); it is never an error and never costs a request.
- `DESCRIBE` of several indices answers one table after another in name order, each table's columns (objects included) in the plugin's own order (the `HashMap` order `SELECT *` uses) with that position as `ORDINAL_POSITION`, up to `size_limit` rows, then stops. The recorded `describe` rows differ from what the first slice answered (it listed leaves only, in name order, with JDBC codes and upper-case type names); this slice matches stock.
- `DESCRIBE ... COLUMN LIKE p` (singular, as `describe-columns` records) filters nothing, as stock does; `COLUMNS LIKE p` filters.
- A backend with no listing (a node without the coordinator role) answers a 400 saying so, as before, and runs nothing.

## Acceptance criteria

- AC-1: over the indices the node lists, `show-tables`, `describe` and `describe-columns` give stock's recorded schema and rows (the cluster column aside; `show-tables-all` lists the recording cluster's own system indices and is checked by pattern shape instead), and `DESCRIBE TABLES LIKE` over a pattern answers every matched index, with `COLUMNS LIKE` filtering.
- AC-2: the literal prefix of a pattern is what is listed: `a%`, `ab_c`, `%` and `%x` list the prefixes `a`, `ab`, `` and `` and the filter then keeps only the matches (a unit test over generated patterns checks `like` against the listing's prefix and that no match is lost to the prefix).
- AC-3: a `SHOW TABLES` costs exactly one LIST and nothing else, and a `DESCRIBE` of one named index lists nothing; both counts are read from the meter (budget row `sql.show_tables.requests`).
- AC-4: a prefix with more than the cap of live names is a 400 that names the cap, and the answer is never a cut listing.
- AC-5: a name the guard refuses is absent from `SHOW TABLES` and `DESCRIBE`, and a statement over names that are all refused answers an empty result, not a 403.
- AC-6: a node whose backend cannot list answers the 400 and makes no request.

## S3 request budget

One LIST per `SHOW TABLES`, on the same derived prefix listing `_cat/indices` makes, so it scales with the names under the prefix (at most the cap) and with nothing else. Idle indices cost nothing. No PUT.

## Crate

`oss-query` gains `meta::prefix_of`, a `describe` over several tables in stock's column order and shape, and an unquoted `LIKE` pattern in the parser (`show tables like logs-%`). `oss-node` gains a `NameLister` the SQL service holds and a descriptor-store implementation of it, mounted where the coordinator feature is on.

## Switch

None. These statements were 400s; no statement that worked changes.

## Risks

- The listing is the one LIST the node already makes for `_cat/indices`; a tenant with many indices under one prefix hits the cap and is told to narrow the pattern.
- A pattern whose literal prefix is empty lists every name up to the cap; that is the stated cost of `SHOW TABLES LIKE %`.
