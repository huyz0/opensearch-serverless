# Mixed fleet: live Java and Rust nodes on one bucket

`crates/node/tests/mixed_fleet.rs` forks the Java shell as a real process beside Rust nodes, on one RustFS bucket ([spec](../../docs/specs/mixed-fleet-soak/SPEC.md)). `WriteKeystore.java` writes the keystore that gives the Java node its S3 credentials (they are secure settings, so not `-D` properties).

Build the shell as `/mnt/project-files/build-setup/README.md` says, with `huyz0/OpenSearch` at `feature/serverlessshell`, then print the production classpath of `:serverless:testkit` (`sourceSets.main.runtimeClasspath`, through a Gradle init script with a task that depends on it so the jars exist) and keep the entries that exist. That is `OSS_JAVA_CP`. `RUSTFS_BIN` is the RustFS binary ([recipe](/mnt/project-files/rustfs/run-recipe-2026-11-21.md) is not in the repo).
