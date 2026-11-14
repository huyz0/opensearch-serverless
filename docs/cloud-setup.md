# Development setup: local and Claude cloud

Nothing needs Docker. Tests run against an in-process S3 emulator; RustFS as a plain binary is optional for wire-level runs.

## Any machine

```bash
scripts/bootstrap.sh
```

It installs what is missing (rustup, the toolchain pinned in `rust-toolchain.toml` with rustfmt, clippy and llvm-tools, and `cargo-llvm-cov`), points git at `.githooks/`, and runs the gate. `--no-gate` installs only.

Requirements: `bash`, `git`, `curl`, Python 3.9 or newer. On Windows use Git Bash; the scripts probe for a working `python3` or `python` because `python3` is often the Store stub there.

## Claude cloud sandboxes

The sandbox is Linux without Docker. Set the environment's setup script to:

```bash
scripts/bootstrap.sh --no-gate
```

The network policy must allow `static.rust-lang.org`, `sh.rustup.rs`, `crates.io`, `index.crates.io`, `static.crates.io` and `github.com` (git dependencies). If the network is closed, commit a `cargo vendor` directory and point `.cargo/config.toml` at it.

## CI

GitHub Actions on plain `ubuntu-24.04` runners, no containers: the gate, and clippy for `aarch64`. See `.github/workflows/ci.yml`.
