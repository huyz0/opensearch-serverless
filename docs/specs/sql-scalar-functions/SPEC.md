# SQL and PPL, slice 3: scalar functions

## Serves

[ADR 0082](../../adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md), order of work item 5 (function library breadth and script pushdown). Builds on [sql-select](../sql-select/SPEC.md), [sql-aggregations](../sql-aggregations/SPEC.md) and [ppl-select](../ppl-select/SPEC.md). The reference is the recorded corpus in `fixtures/sql` (stock 3.10.0): `scalar-upper`, `scalar-arith`, `scalar-abs`, `scalar-concat`, `scalar-case`, `scalar-if`, `scalar-ifnull`, `scalar-year`, `scalar-date-format`, `scalar-where`, `where-in-expression`, `select-function-no-from`.

## Change

What the recordings show about stock, which sets the shape of this slice:

- A scalar function in the **select list** is not pushed. The recorded `_explain` bodies of `scalar-upper`, `scalar-arith` and `scalar-case` are a plain `_source` include of the fields the expression reads (`"includes": ["name"]`); the plugin evaluates the expression over each returned row.
- A function in **`WHERE`** is pushed as `{"query": {"script": {"script": {"source": "{\"langType\":\"v2\",\"script\":\"<Java-serialised expression>\"}"}}}}`. That blob is a Java object graph and is not reproduced; the equivalent here is a `script` query in Painless source, run by the Painless front end of [ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md).

So this slice has two lowerings, over one expression tree in the `Select` AST (`Expr`: field, literal, function call, binary arithmetic, `CASE`):

1. **Projection.** The planner adds the fields an expression reads to `_source.includes` and records the expression on the column; `rows` evaluates it over each hit's `_source`. It reads only rows the search already returned (at most `size_limit`), so no request is added and nothing scales with documents. A null input gives null.
2. **Predicate.** A `WHERE` condition with an expression operand lowers to one `script` query: Painless `source` generated from a fixed table of functions, literals passed as `params` (never spliced into the source), added to the `bool.filter` beside the other conditions. A function with no Painless form in the table is a 400.
3. **No `FROM`.** `select abs(-3)` is evaluated by the planner with no search and no mapping read.

Functions in this slice: `abs`, `upper`, `lower`, `length`, `concat`, `ifnull`, `if`, `case when ... then ... else ... end`, `year`, `month`, `date_format` (the `%Y %m %d %H %i %s` specifiers), and `+ - * / %`. Each has a result type matching the recorded schema (`keyword`, `integer`, `long`, `double`, `timestamp`). Anything else keeps today's refusal text, `the function [x] is not supported yet`. PPL gets the same expressions in `where`; PPL `eval` stays refused (it adds columns that later commands name).

An expression in `ORDER BY` or `GROUP BY` stays refused: ordering by one needs a script sort and grouping by one a script composite source, both later slices.

## Acceptance criteria

- AC-1: over the recorded documents each `scalar-*` and `select-function-no-from` statement in the list above gives stock's `datarows` and `schema`, compared by `crates/query/tests/recorded.rs`; a statement still outside the slice is on the `LATER` list and a listed statement that starts working fails the test.
- AC-2: a select-list expression adds only its field names to `_source.includes`, the body has no `script` key (the script-free rule of `oss-query` holds for projections; predicates are the one place this slice puts a `script` key, AC-3), and the planner makes the same single search as the plain column select (compared body for body, `size` and `from` included).
- AC-3: a `WHERE` expression lowers to a `script` query whose `source` is generated from the function table and whose literals are all in `params`: a literal holding quotes, backslashes and `*/` leaves `source` unchanged (a fixed-seed test over generated literals).
- AC-4: `scalar-where` and `where-in-expression` give stock's rows through the real reader and Painless runtime on the emulator, in `crates/node/tests/it`; a function the front end refuses is a 400 with the plugin's `SemanticCheckException` shape and no search runs.
- AC-5: no expression shape makes the planner issue a second search or read documents to filter them in memory: a `WHERE` expression is always the `script` query, never a post-filter; a test over every function in the table checks the body and the request count (the cold count equals the plain select's).
- AC-6: arithmetic follows stock's types (`age + 1` is `integer`, `/` of two integers is integer division as recorded), division by zero is null as stock gives, and an overflow is null rather than a panic; the evaluator is checked by a fixed-seed property test against the Painless runtime for the same expression.
- AC-7: the parser bounds hold with expressions: nesting depth, token count and identifier length refuse before any allocation in proportion to them, and the fixed-seed mutation fuzz over a corpus with expressions never panics.

## S3 request budget

Unchanged: a statement costs the search it pushes. A projection adds none; a predicate script is part of that same search (the Painless compile is in memory and cached per source). Budget rows `sql.select.cold.requests` and `sql.select.warm.requests` gain a function-bearing statement in the same test (AC-5).

## Crate

`oss-query` gains `expr.rs` (the tree and its evaluator), `expr_parser.rs` and `expr_script.rs` (the Painless generator). `oss-node` changes only to pass the expression results through; no route, setting or crate is added.

## Switch

None. Statements with functions were 400s; no statement that worked changes.

## Risks

- The Painless front end is the dependency of AC-4: a function it cannot compile today is refused, and the table lists only those it can; the spec does not widen the front end.
- Date functions depend on the time-zone and format rules of the recorded corpus; `date_format` supports the listed specifiers only and refuses others.
- The in-memory evaluator and the Painless text can disagree; AC-6 holds them together by property test, not by shared code.
