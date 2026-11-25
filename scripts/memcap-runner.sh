#!/usr/bin/env bash
# Cargo target runner (CARGO_TARGET_<TRIPLE>_RUNNER): runs a test binary with a
# cap on its address space, so a test, or a mutant, that allocates without
# stopping fails with an allocation error in its own process instead of taking
# the runner down. A mutated loop bound in a decoder pushes into a Vec until
# the runner's 16 GB are gone and GitHub reports "The runner has received a
# shutdown signal". The test is then failed, which is what a
# mutant must do to count as caught.
#
# MEMCAP_KIB sets the cap in KiB (default 2 GiB per test process; nextest runs
# one process per test, four at a time on the runner). Linux only.
set -eu
ulimit -v "${MEMCAP_KIB:-2097152}"
exec "$@"
