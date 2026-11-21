# Commit checks at the pull request

## Serves

The process rules in the `git-workflow` skill. Every PR is squash-merged, so the commits inside it never reach `main`; only the squash message does. A per-commit rule in CI cost a force-push each time a fix commit lacked a `Spec:` trailer or a thread merged `main` into its branch.

## Change

CI stops checking the subject format and the `Spec:` trailer on every commit, and checks them where the squash message comes from: the PR title and body.

- `scripts/check-commit-msg.sh` gains `--ci`, which runs only the no-AI-attribution rule. The `commit-msg` hook still runs every rule at commit time, where the author is already editing the message.
- CI runs `--ci` over `git rev-list --no-merges`, so a merge commit is never checked, whatever its subject.
- `scripts/check-pr.py` gains `--title`. The title must be `type(scope): lowercase description`; for feat, fix and perf the body needs a line `Spec: docs/specs/<feature>/SPEC.md` (an existing file) or `Spec: none (<reason>)`.
- Attribution stays checked on every commit: it is cheap to avoid and a squash message may include the commit bodies.
- Contributors rebase onto `main` rather than merge it (`git-workflow` skill).

Does not change: the subject and `Spec:` rules themselves, the hook, the PR sections, risk routing.

## Acceptance criteria

- AC-1: `check-commit-msg.sh --ci` refuses a message with AI attribution.
- AC-2: `check-commit-msg.sh --ci` accepts a fix commit with no `Spec:` trailer and a subject not in `type(scope)` form.
- AC-3: `check-commit-msg.sh` without `--ci` still refuses a fix commit with no `Spec:` trailer.
- AC-4: `check-pr.py --title` refuses a title not in `type(scope): lowercase` form.
- AC-5: `check-pr.py --title` refuses a feat, fix or perf title whose body has no `Spec:` line, and one naming a file that does not exist.
- AC-6: `check-pr.py --title` accepts a docs title with no `Spec:` line and a fix title with `Spec: none (<reason>)`.
- AC-7: the CI commit step excludes merge commits.

## Test plan

AC-1 to AC-6 are planted-violation cases in `scripts/selftest-checks.py`, each seen to fail before the change. AC-7 is read from `.github/workflows/ci.yml` (`--no-merges`).

## S3 request budget

Unchanged: no store code touched.

## Switch

None: it removes cost with no downside to the product; the rules are unchanged and moved to where they take effect.

## Risks

A fix PR whose body lacks `Spec:` is now caught at the PR, not at the commit; the author edits the description without a push. A squash message built by hand without the `Spec:` line is not checked; the title and body are what GitHub proposes.
