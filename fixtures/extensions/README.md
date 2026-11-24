# Extension pins a Rust node wrote

`pinned-descriptor.bin` is the descriptor register value of the index `logs` that a Rust create wrote with one analyzer pin (`ext_stem`, [ADR 0053](../../docs/adr/0053-the-analyzer-extension-point.md)); `a_pinned_descriptor_is_what_the_java_fixture_holds` (`crates/node/tests/server/index_pins.rs`) rewrites it with `OSS_WRITE_FIXTURES=1` and otherwise checks it still holds what a create writes.

`ServerlessPinnedDescriptorRefusedTests.java` is a test for the shell's `:serverless:testkit` module (copy it to `serverless/testkit/src/test/java/org/opensearch/serverless/testkit/`). It reads the descriptor as a Java node does and builds its analyzers, which must fail with `Unknown analyzer type [serverless_module] for [ext_stem]`. Run `gt.sh :serverless:testkit:test --tests "*ServerlessPinnedDescriptorRefusedTests" -Dtests.rust.pinned=<path>/pinned-descriptor.bin -Dtests.security.manager=false`; the build recipe is in `fixtures/mixed-fleet/README.md`.

Run on the shell at `feature/serverlessshell` (`e71d4233`): PASS, `Unknown analyzer type [serverless_module] for [ext_stem]`. It builds the analyzers through the core registry the shell's `SharedAnalysis` uses; it does not start a shell node. The pin setting stays off until the rest of `docs/specs/extension-analyzer/VERIFIED.md` is done.
