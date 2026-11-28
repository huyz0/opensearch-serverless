# PPL, slice 3: eval

## Serves

[ADR 0082](../../adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md), order of work item 5. Builds on [ppl-select](../ppl-select/SPEC.md) and [sql-scalar-functions](../sql-scalar-functions/SPEC.md). The reference is the recorded `ppl-eval` (stock 3.10.0): `source=compat-sql | eval double_age = age * 2 | fields name, double_age`.

## Change

`eval <name> = <expression> [, <name> = <expression>]...` adds computed columns. The expression is the one the SQL select list takes (`abs`, `upper`, `lower`, `length`, `concat`, `ifnull`, `year`, `month`, `date_format`, `if`, `case`, and `+ - * / %`), evaluated in memory over the rows the search returns, so it adds no request and no `script` key to the body. A `fields` command after it may name the computed columns; with none, the answer is every field followed by the computed ones, in the order written. An arithmetic column is `bigint` in PPL, as recorded.

A computed name cannot yet be used by a later `where`, `sort` or `stats`: those would need a script, a later slice, and are refused as an unknown field. `dedup` and `parse` stay on the later list.

## Acceptance criteria

- AC-1: `ppl-eval` gives stock's rows and schema over the recorded documents (`crates/query/tests/recorded_ppl.rs`, no longer on its `LATER` list).
- AC-2: an `eval` adds only the fields its expression reads to the search body's `_source`, and the body is the plain select's apart from that (compared body for body, no `script` key).
- AC-3: with no `fields`, the columns are every field then each computed column in order; a repeated name keeps the last expression; a computed column shadows nothing in the mapping unless it reuses a field's name, in which case the later `fields` item is the computed one.
- AC-4: a computed name used in `where` or `sort` is a 400 and no search runs.

## S3 request budget

Unchanged: a statement costs the search it pushes.

## Switch

None. These statements were 400s.

## Risks

- The in-memory evaluator is the SQL one, so it carries the SQL evaluator's null and type rules; PPL's own typing of `/` (integer division) is not asserted beyond the recorded case.
