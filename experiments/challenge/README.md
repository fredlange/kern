# Challenges: frozen material

> **2026-11-04, layout note (added after the runs; nothing below is changed):** this directory moved from
> `challenge/` to `experiments/challenge/`, and the plan to [docs/research/CHALLENGE.md](../../docs/research/CHALLENGE.md).
> The scripts expect the old layout; to rerun one, check out the commit its results name.

See [docs/CHALLENGE.md](../docs/CHALLENGE.md) for the plan. Everything in this directory is
committed **before** the challenge it belongs to runs; results go to CHALLENGE.md's Results
section with the freeze commit's hash.

## Thresholds, frozen 2026-11-03 (Fredrik: "freeze defaults")

### C1, seeded bugs
- **P1:** Kern's proofs and examples (the mechanisms `proof` and `eg`) catch ≥ 90% of the
  live bugs that land in a Kern **body** (catalogue + blind).
- **P2:** Kern catches strictly more live bugs than TypeScript's tests, and strictly more
  than Go's.
  - "Kern catches" = `kern check` is not clean, by any mechanism, including the ratchet.
  - The set is every live bug that is expressible in Kern. Bugs that land in a Kern spec
    `need` are included and count as caught only if `check` is not clean.
  - Bugs not expressible in Kern are excluded from all three counts and reported.
- **P3, a finding rather than a pass condition:** if Kern's P1 rate on the blind bugs is
  more than 15 points below its rate on the catalogue, author bias is real.
- **Live:** the mutated Kern, TypeScript and Go agree with each other on all 5,000 cases
  (seed 1), and differ from the original on at least one.
  - Disagreement between the three is a translation error, fixed in the patch and
    counted.
  - A bug that changes nothing is replaced from the reserve list in `c1/bugs.md`, and
    counted.

### C6, prover stress
- **P1:** at d=6, k=4, e=6 (n=10), `check` (full, with mutation testing) takes ≤ 1 s per
  definition.
- **P2:** at n=200, d=3, k=2, e=3, the whole `check` takes ≤ 60 s.
- **Also reported:** curves over d (1–8), k (0–6), e (0–8), n (10/50/200), state with
  invariants on or off, peak memory, and whether an unchanged definition is re-proven
  (expected: yes, a gap).
