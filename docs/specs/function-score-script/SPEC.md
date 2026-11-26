# `function_score` script_score in the expression language

## Serves

R14.6's remaining gap, "runs no script". Stock `function_score` takes `script_score` (`ScriptScoreFunctionBuilder`). The only script language this node can run is `expression` (lucene-expressions, native Rust): the Painless runtime in lucene-rust executes an IR produced by the Java front end, so a node cannot compile Painless source. This change serves `expression` and refuses every other language with a stated reason.

## Change

- `oss_reader::dsl::queries::function_score` parses `{"script_score":{"script":<script>}}` as a function, alone or in `functions`, under the same `filter`, `weight` and modes as the other functions. `<script>` is a string, or `{"source"|"inline":..,"lang":"expression","params":{..}}`; a stored script (`{"id":..}`) is refused.
- The expression is compiled once per request by lucene-expressions. Its variables are `_score`, `doc['field'].value` for a numeric or date field with doc values, and `params.name` for a number in `params`. Anything else is a 400 `script_exception` naming the variable.
- No loop, no allocation and no recursion exist in the language, so a script's cost is its size, bounded by the 65,535-byte source limit; nothing reaches the heap per document.
- A script is evaluated only on documents the query matched; the result is the function's score, clamped as stock does (a negative result is a 400 `illegal_argument_exception` as stock raises).
- **Refused:** `lang` other than `expression` (a missing `lang` is `painless` in stock, so a bare string is refused: 400 "script_score runs only lang [expression] on this node"), a stored script, a multi-valued or text field, `doc['f'].size()` and the other doc-value methods, `Math.*` calls outside the expression function table.
- No new setting: the change only adds an accepted shape, with no measured downside.

## Acceptance criteria

- AC-1: an expression over `_score`, a numeric doc value and a param scores the way the same arithmetic does in `field_value_factor`'s equivalent, hit for hit.
- AC-2: `script_score` combines with `filter`, `weight`, `score_mode` and `boost_mode` as the other functions do.
- AC-3: every refusal above answers 400 with the stated reason and runs nothing.
- AC-4: a search with a script costs the same object-store requests as the same search without one.

## Tests

Unit: `crates/reader/src/dsl/queries/function_score_tests.rs`. Integration: `crates/node/tests/it/function_score_script.rs`.

## S3 request budget

None added: doc values are read as for any scored search; the expression compiles in memory.
