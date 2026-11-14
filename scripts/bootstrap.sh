#!/usr/bin/env bash
# One command from a fresh clone to a green gate, on Linux (including Claude
# cloud sandboxes), macOS, and Windows under Git Bash. No Docker anywhere:
# tests use an in-process S3 emulator, never a container.
#
#   scripts/bootstrap.sh          install what is missing, then run the gate
#   scripts/bootstrap.sh --no-gate   install only
#
# What it installs, each only when absent: rustup and the toolchain pinned in
# rust-toolchain.toml (with rustfmt, clippy, llvm-tools), cargo-llvm-cov, and
# the repo's git hooks.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
. scripts/lib/py.sh

say() { echo "bootstrap: $*"; }

if ! command -v cargo >/dev/null 2>&1; then
  if ! command -v rustup >/dev/null 2>&1; then
    say "installing rustup (minimal profile)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
  fi
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

# `rustup show` installs the toolchain rust-toolchain.toml pins, with its
# components, when it is missing.
say "toolchain: $(rustup show active-toolchain 2>/dev/null || true)"
rustup show >/dev/null

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  say "installing cargo-llvm-cov"
  cargo install cargo-llvm-cov --locked
fi

py -c 'pass' || { say "python 3.9+ is required for the check scripts"; exit 1; }

if git rev-parse --git-dir >/dev/null 2>&1; then
  scripts/setup-hooks.sh
fi

if [ "${1:-}" != "--no-gate" ]; then
  say "running the gate"
  scripts/gate.sh
fi
say "done"
