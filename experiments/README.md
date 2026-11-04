# Experiments

Measurements behind Kern's design claims. None of this ships, and nothing in `kern/`, `runtime/` or
`tests/` imports from here. Each experiment's report is in [docs/research/](../docs/research/).

| Directory | What it measured | Report |
|---|---|---|
| `challenge/` | pre-registered challenges: C1 seeded bugs, C2 a cold agent author (Kern views vs full source vs TypeScript), C6 prover stress | [CHALLENGE.md](../docs/research/CHALLENGE.md) |
| `compare/` | Kern vs behaviour-identical TypeScript and Go: tokens to write and read, and a differential test of the three | [COMPARE.md](../docs/research/COMPARE.md) |
| `bench/` | the runtime baseline: the same workload in Kern's host, Python, Go, C and Rust | [RUNTIME_PLAN.md](../docs/research/RUNTIME_PLAN.md) |
| `scale/` | the scale scorecard over the escrow app's three sizes | [SCALE.md](../docs/research/SCALE.md) |

**Frozen material.** The challenge files were committed before their runs, and C2's are hashed
(`challenge/c2/HIDDEN.sha256.json`). They are kept exactly as run. Their scripts find the repository
from their own location and expect the layout of that time (`challenge/`, `compare/` at the
top level), so they don't run from here. To rerun one, check out the commit its report names
(for C2, `d521236`).
