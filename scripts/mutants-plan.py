#!/usr/bin/env python3
"""Which slice of a pull request's mutants CI tests.

  mutants-plan.py COUNT BUDGET SEED   prints `k/n`, or nothing when COUNT is 0
  mutants-plan.py --self-test

A PR job tests at most BUDGET mutants, so its cost does not grow with the
diff. With COUNT mutants in the diff, they split into n = ceil(COUNT/BUDGET)
shards and CI tests shard k, chosen from SEED (the head commit), so each push
of a large PR samples a different slice. Shards are 0-based, as cargo-mutants'
`--shard k/n` counts them. The nightly run tests every mutant. Idea from
pstore's mutants-plan.py.
"""
import sys


def plan(count, budget, seed):
    if count <= 0:
        return None
    n = max(1, -(-count // max(1, budget)))
    k = int(seed, 16) % n if seed else 0
    return f"{k}/{n}"


def self_test():
    cases = [
        ((0, 40, "ab"), None),
        ((1, 40, "ab"), "0/1"),
        ((40, 40, "ff"), "0/1"),
        ((41, 40, "0"), "0/2"),
        ((41, 40, "1"), "1/2"),
        ((559, 40, "a1719ec"), f"{int('a1719ec', 16) % 14}/14"),
    ]
    bad = [f"plan{args} = {plan(*args)}, expected {want}" for args, want in cases if plan(*args) != want]
    print("\n".join(bad) if bad else f"mutants-plan: self-test ok ({len(cases)} cases)")
    return 1 if bad else 0


def main():
    if "--self-test" in sys.argv:
        return self_test()
    count, budget, seed = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
    shard = plan(count, budget, seed)
    if shard:
        print(shard)
    return 0


if __name__ == "__main__":
    sys.exit(main())
