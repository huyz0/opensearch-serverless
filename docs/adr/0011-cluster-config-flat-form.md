# 0011: The cluster config register written in the flat settings form

**Status:** Accepted after independent review, 2026-11-16.

## Context

The Java shell keeps an operator's persistent cluster settings in one register, `<base>cluster/config/settings` (`ClusterConfig.java`). It writes the value with `Settings.toXContent`, which nests keys into objects (`{"cluster":{"routing":{...}}}`). Each object's fields come in the order of a `HashMap`: an order set by `String.hashCode`, the map's capacity and its resize history. It reads the value back with `Settings.fromXContent`, which joins nested keys with `.` and accepts flat keys as well. The same parser reads `toXContent`'s `flat_settings` form: one field a dotted key, in `TreeMap` order.

A Rust node must read and swap this register beside Java nodes. The rule for `oss-format` is that bytes on disk match the Java's. Reproducing the nested form byte for byte would mean porting `HashMap`'s iteration order. That order is not part of any contract and can change with a JDK. The bytes carry no checksum or hash, and nothing compares them.

The register holds scalars only: `ClusterConfig`'s contract, which the Java's REST handler enforces by refusing, with a 501, any change that builds a list, `p.0` keys included. The Java puts no bound on the value's size.

## Decision

- Rust reads the register exactly as `Settings.fromXContent` and `Settings.Builder.build` do:
  - nested and flat keys alike;
  - numbers as their text;
  - `null` as absent;
  - `p.0`, `p.1` ... made a list `p`, with the builder's conflict refused.

  The `p.0` rule is checked against OpenSearch 3.3.0's own `Settings` (`fixtures/cluster-config/`).
- Rust writes the `flat_settings` form: one field a key, in key order, with Jackson's escaping. For keys whose UTF-8 and UTF-16 orders agree, this is byte for byte what `toXContent` writes with `flat_settings=true`.
- Rust refuses an update whose changes build a list, as the Java's handler does, with its message. A list already in a register reads, and an unrelated update keeps it.
- Rust bounds the settings at `MAX_CLUSTER_SETTINGS_BYTES`, 256 KiB of flat text. Nested keys repeat in the flat form, so a short nested value can stand for a large flat one. The bound is counted while reading, so a crafted value cannot make a read allocate past it. An update that would pass it is refused.
- Rust refuses an update that would leave a `p.0` beside a `p`. The Java's builder refuses those, and so would every Java read after it. Rust also refuses settings the Java holds but cannot read back, a null `p.0` or a list made from `p.0.0`, so it never writes a value a Java node cannot read.
- The Java shell does not change: it reads the flat form today, and its own writes are read by Rust.

## Consequences

- A register the Rust wrote and one the Java wrote hold different bytes for the same settings. Both runtimes read either as the same settings, and a compare-and-swap compares generations, never bytes.
- The flat form is longer than the nested one wherever keys share a prefix. A Java-written value that fits a caller's buffer can therefore need a larger buffer to write back, and the bound is on the flat form for that reason. Settings past it read nowhere in Rust, where the Java would read them.
- The value written is deterministic, so a test pins it, and so does any later byte-level check.
- Keys outside the BMP sort differently in UTF-8 and UTF-16, so for those the Rust bytes are not the Java's `flat_settings` bytes. They still read as the same settings.
- If the Java ever starts hashing or comparing the value's bytes, this decision has to be revisited.
