#!/usr/bin/env bash
# Validates one commit message file. Used by .githooks/commit-msg and by CI
# for every commit in a pull request.
#
#   - Subject: `type(scope): lowercase description`.
#   - feat, fix and perf commits carry a `Spec:` trailer naming an existing
#     docs/specs/<feature>/SPEC.md, or `Spec: none (<reason>)` for a change
#     too small to need one.
#   - No AI attribution: no Co-Authored-By naming Claude or Anthropic, no
#     Claude-Session trailer, no "Generated with Claude" footer (the owner's
#     rule for every repository in the project).
#
# Portable: Linux, macOS (bash 3.2) and Windows Git Bash.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
msg_file="$1"
subject="$(head -n1 "$msg_file")"

if printf '%s' "$subject" | grep -qE '^(Merge|Revert) '; then
  exit 0
fi

pattern='^(feat|fix|docs|test|chore|refactor|perf|build|ci)(\([a-z0-9-]+\))?: [a-z].*'
if ! printf '%s' "$subject" | grep -qE "$pattern"; then
  echo "commit-msg: subject must be 'type(scope): lowercase description'" >&2
  echo "  type is one of feat|fix|docs|test|chore|refactor|perf|build|ci" >&2
  echo "  got: $subject" >&2
  exit 1
fi

if printf '%s' "$subject" | grep -qE '^(feat|fix|perf)'; then
  spec="$(grep -E '^Spec: ' "$msg_file" | head -n1 | sed 's/^Spec: //' || true)"
  if [ -z "$spec" ]; then
    echo "commit-msg: feat, fix and perf commits need 'Spec: docs/specs/<feature>/SPEC.md' or 'Spec: none (<reason>)'" >&2
    exit 1
  fi
  if ! printf '%s' "$spec" | grep -qE '^none \(.+\)$' && [ ! -f "$spec" ]; then
    echo "commit-msg: Spec: '$spec' is not an existing file or 'none (<reason>)'" >&2
    exit 1
  fi
fi

if grep -qiE '^Co-Authored-By:.*(claude|anthropic)|^Claude-Session:|Generated with \[?Claude' "$msg_file"; then
  echo "commit-msg: no AI attribution (Co-Authored-By: Claude, Claude-Session, 'Generated with Claude')" >&2
  exit 1
fi
