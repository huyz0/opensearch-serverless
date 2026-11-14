# shellcheck shell=bash
# Sourced by scripts that run Python. On Windows `python3` is often the
# Microsoft Store stub, which exists on PATH and fails when run; probe that a
# candidate actually runs instead of trusting `command -v`.
py() {
  local cand
  for cand in python3 python; do
    if "$cand" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 9) else 1)' >/dev/null 2>&1; then
      "$cand" "$@"
      return
    fi
  done
  echo "no working python >= 3.9 on PATH" >&2
  return 1
}
