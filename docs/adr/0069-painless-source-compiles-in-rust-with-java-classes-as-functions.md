# 0069: Painless source compiles in Rust, and Java classes are Rust functions

**Status:** Accepted

## Context

The Rust node runs `lang: expression` only. lucene-rust's `lucene-painless` crate runs Painless, but only from the IR that Painless's own Java front end writes (`PIR2`: every type, cast, method and `def` site already resolved). A node without a JVM cannot compile Painless source, so `script_score`, scripted sort, `script_fields`, the ingest `script` processor and stored scripts are refused or run on a Java node.

Stock Painless gives a script the Java object model: `String`, `ArrayList`, `HashMap`, `Pattern`/`Matcher`, `StringBuilder`, `Optional`, streams, and `java.time` (`ZonedDateTime`, `Instant`, `Duration`, `ChronoUnit`, ...). The runtime already implements most of this as Rust values behind name-based dispatch; `java.time` is the part that is a real object graph.

The project owner's direction: support the Painless **grammar**, and replace every Java object with **Rust functions over plain values**, not objects (dates are the example: functions over epoch values, not a `java.time` model).

## Decision

### Where it lives

A front end in lucene-rust, `lucene-painless/src/front/`: `lex`, `parse` (AST), `analyze` (types, casts, `def`), `lower` (AST to the existing `ir::Script`). `compile`, `specialize`, `check`, `typed` and the VM stay the only execution path; they gain the pieces named under **Runtime changes**. The node calls `front::compile(source, context) -> ir::Script | Refusal`. Java and Rust front ends feed the same IR, so a Java-written blob still runs.

### What the IR needs, and where it comes from

The IR is fully resolved: a `Method` carries its target class, declaring class, parameter and return types, and a `Cast` its box and unbox classes. So the front end carries **Painless's signature table**, generated into lucene-rust from OpenSearch's allowlist files (`spi/*.txt`) by a script, with a tree check that the generated file is current. The table is the typing source for the front end only. At run time `InvokeCall` still dispatches by (class, name, arity) or by the receiver's value into the Rust functions in `api/`, so nothing new is called by reflection. "Java class" survives only as a namespace label in the table; no class is instantiated.

`check` compares, per `def` site, Painless's answer per native class (`Script::sites`, `resolves`) with the native one. The front end writes `resolves` from the **allowlist table**, never from the native function table, so the check stays an independent comparison and the `unsound = 0` guard keeps its meaning.

### Grammar

The whole of `PainlessParser.g4` is parsed: declarations, `def`, all operators and compound assignments, `?:`, `?.`, `instanceof`, casts, `for`/`for-each`/`while`/`do`, `break`/`continue`, `try`/`catch`/`throw`, user functions, lambdas, method references, array/list/map initializers, regex literals, string concatenation, `new`. A construct the analyzer does not yet serve is refused after parsing, naming it and its offset, never skipped.

### Java classes are functions, not objects

The front end has no class model. A call `recv.name(args)` or `Class.name(args)` is looked up in a **function table** keyed by (value kind, name, arity) and lowers to a call of a Rust function taking the receiver as its first argument. There is no object identity, no `java.time` instance, no `Class`, no reflection, no user-visible constructor of a library object beyond the literals below.

- **Values:** the runtime's own `Value`s: numbers, `boolean`, `String` (UTF-16 semantics kept), `List`, `Map`, `Set` as value collections, `null`.
- **Surface served as functions:** `String`/`CharSequence`/`StringBuilder` methods, `Math`/`StrictMath`, `Integer`/`Long`/`Double`/`Boolean`/`Character` statics, `List`/`ArrayList`, `Map`/`HashMap`, `Set`, `Arrays`, `Collections`, `Objects`, `Comparator` helpers, `Pattern`/`Matcher` (as functions over a pattern string and a subject; no `Matcher` object: `matches`, `find`, `replaceAll`, `group` over an index), lambdas and functional helpers over lists and maps (`forEach`, `removeIf`, `sort`, `computeIfAbsent`, ...).
- **Dates:** a date is a distinct runtime value kind `Date` (epoch milliseconds, UTC), not a `long`, so `def` sites can key on it and `long x = doc['d'].value` is refused at compile time as stock refuses it. A `Date` returned from `script_fields` renders as the ISO-8601 string stock gives. `Date` is mapping-independent: `doc['d'].value` is a `def` site in the IR (as in Painless), and the field's kind is resolved by `specialize` at query time, so the compile cache needs no mapping in its key. The existing `ZonedDateTime`/`Instant`/`Duration` objects stay in `api/time.rs` for Java-written IR only and are unreachable from source compiled here. Functions over a `Date`: `getYear`, `getMonthValue`, `getDayOfMonth`, `getDayOfWeekValue`, `getDayOfYear`, `getHour`, `getMinute`, `getSecond`, `toEpochMilli`, `plusDays|Hours|Minutes|Seconds|Millis`, `minus*`, `truncatedTo*`, `daysBetween`, `millisBetween`, `isBefore`, `isAfter`. An enum-valued stock result (`getDayOfWeek()`, `getMonth()`) is an `int` carrying `getValue`, `name` and `toString`. A duration is a `long` of milliseconds. Fixed offsets only; region zones, `DateTimeFormatter`, `LocalDate`/`LocalDateTime`/`Period` and `ChronoField` objects are refused (a later ADR may add formatting as plain functions).
- **Refused, as a class of constructs:** streams, `Optional`, `Class` and reflection, object identity (`==` on two `new`ed library objects), anything needing a Java object whose behaviour is not a pure function of its values.

### Contexts and bindings

`_score`, `doc['f']`, `params`, `_source`-free contexts first: `score`, `filter`, `sort` (number), `aggs` value, `field` (`script_fields`), `ingest`. `doc['f']` keeps `check`'s static field resolution; a field without a native reading is refused as today.

### Bounds (per script, not per document)

- **Source** at most 65,535 bytes; parse depth at most 100; a script over **20,000 IR nodes** after lowering is refused.
- **Loops:** the existing per-function loop counter (`max_loop_counter`, counted per loop iteration), default 1,000,000 as Painless's; exceeding it fails that document's invocation with Painless's error. Regexes run on the linear-time `regex` engine, so no backtracking bound is needed; a pattern it cannot match exactly is refused.
- **Calls:** depth at most 128.
- **Memory:** every allocating function (string build, list/map growth, `new`) charges a per-invocation arena reserved from the node's memory pool before allocation (ADR 0004): default 4 MiB per invocation, 1 MiB per single string or collection. The arena resets after each document, so nothing accumulates across documents. A script that exceeds it fails with a `script_exception`, not an OOM. A string concatenation is charged at its result size. This arena is **new runtime work** (the VM has none today); it lands in slice 4 with its own tests.
- **Cache:** compiled scripts are plain `Send` data (`ir::Script`), cached on the node in a bounded LRU (256 entries, at most 4 MiB charged to the memory pool) keyed by (source, context, loop and memory limits). Each thread builds its own `Program` from the cached IR (a `Program` is `Rc`-based and not shared).
- **Determinism:** `Math.random`, `System.currentTimeMillis`/`nanoTime` and any other non-pure function are refused in the `score`, `filter`, `sort` and `aggs` contexts; `random_score` stays its own function. A script is a pure function of its document, `_score` and `params`.
- **Security:** source is untrusted. Nothing reaches I/O, reflection, the clock, threads or the environment: the function table is closed and generated from the allowlist, and every bound above applies. Limits are knobs with stock's defaults (not switches that change behaviour).
- **No S3 request** is added: compilation is in memory and the compiled script is cached, so a search with a script costs the same requests as without.

### Refusal

An unsupported grammar construct, class, method or type is a 400 `script_exception` with stock's `compile error` reason and `script_stack`, whose `caused_by` names the construct and offset as unsupported, raised at compile time before any document is read. A script is never run with different meaning than stock: when the front end cannot give stock's answer, it refuses. Where the project owner chose a function in place of an object (dates, enums, durations), the difference is listed in `docs/compatibility.md` under **Differs**.

### Switch

`serverless.scripting.painless.native_compile` (`OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE`), default **off** until the corpus floor and the bounds hold; off, a Painless script is refused as today, byte for byte. Plus `...max_loop_counter`, `...memory_per_invocation`. Default flips on only when AC-4 to AC-7 of the spec pass.

### Verification against stock

lucene-rust's `tests/data/corpus.bin` holds 4,964 scripts from Painless's own test suite with each source and Java's result or exception. The front end's results must equal Java's on every script it accepts, except scripts that use a replaced object (dates, enums, durations), which are checked against answers recorded from stock OpenSearch for the same epoch values. The accepted share has a floor that only rises, a script it accepts must never return a different value (`unsound = 0`), and scripts it refuses are counted by reason. The corpus holds the language, not the contexts (it has no `doc`, `_score` or `params`); the contexts are checked on lucene-rust's `fixtures/data/painless-score` scripts (score, filter, sort, aggregation) and on the node's own integration tests. The corpus records only that Java did not compile a script (status 2), not why, so the compile check is one-way: the front end accepts nothing Java rejected.

## Runtime changes

Not "unchanged": a `Date` value kind and its functions in `api/`, the per-invocation allocation arena, and the signature table. Each is its own slice with tests.

## Alternatives rejected

- **Port Painless faithfully, objects included.** Largest build; reproduces `java.time` and reflection-shaped objects the owner does not want.
- **Hand-picked small grammar.** Quick, but stock scripts fail on first use of a lambda or `for-each`, and the refusal set is unprincipled.
- **Embed a JVM / keep Java nodes for scripts.** Breaks the Rust node's reason to exist.

## Consequences

Slices, each its own PR: (1) lexer, parser, AST; every corpus source parses or is refused as Java refuses it. (2) analyzer and lowering to IR for the core types. (3) the function table, one class family per PR, with Java-recorded answers. (4) bounds and the setting. (5) node wiring: `script_score`, scripted sort, `script_fields`, ingest `script`. Cost: a second front end beside Java's, held to Java by the corpus. Risk: a typing rule differs from stock in a case the corpus lacks; the refusal rule and the unsound count are the guard.
