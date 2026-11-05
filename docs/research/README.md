# Research notes

Plans, pre-registered scorecards and results: the record of how Kern was designed and measured.
The user documentation is one level up ([REFERENCE.md](../REFERENCE.md), [GUIDE.md](../GUIDE.md),
[HOST.md](../HOST.md)).

| Note | About |
|---|---|
| [SHORTCOMINGS.md](SHORTCOMINGS.md) | the one register of what Kern doesn't do well yet or hasn't shown, and what "fixed" means for each |
| [AUDIT.md](AUDIT.md) | the audit against [FUNDAMENTALS.md](../FUNDAMENTALS.md), with ranked gaps |
| [FIXES.md](FIXES.md) | the fix pass, with before/after evidence per step |
| [SCALE.md](SCALE.md) | one app in three sizes, against a pre-registered scorecard |
| [COMPARE.md](COMPARE.md) | Kern vs TypeScript and Go |
| [CHALLENGE.md](CHALLENGE.md) | the pre-registered challenges, per bet |
| [ROADMAP_RUN.md](ROADMAP_RUN.md) | the roadmap run: conditions and results per step (MCP, loops, handlers, fuzzer, packages) |
| [HOST_PLAN.md](HOST_PLAN.md) | the host proof of concept and its scorecard |
| [RUNTIME_PLAN.md](RUNTIME_PLAN.md) | compiling Kern to Rust: plan, scorecard, results |

**Layout change, 2026-11-04.** These notes were written before the repository was reorganised.
Paths in their text now point at the new places (`validation/`, `experiments/`, `runtime/` for the
crate, `kern/emit_rust.py`). Commands quoted inside results ran from the old layout.
