# AGENTS.md: working in the Kern repo

Kern is a language for agent-written code. Agents read compact, machine-checked views
instead of bodies, and humans review a generated proof page instead of diffs. This file is
for any agent working here.

## Read first

1. [docs/REFERENCE.md](docs/REFERENCE.md): the language (about 1,900 tokens).
2. [docs/GUIDE.md](docs/GUIDE.md): the tools, file formats and Python API, and how to read
   `check` output.
3. [CLAUDE.md](CLAUDE.md): the design decisions, measurements and known limitations. Skim it.
   `docs/HOST.md` is the host's design; `docs/research/` is history and evidence (AUDIT, FIXES,
   SCALE, COMPARE, CHALLENGE, ROADMAP_RUN, the plans).

Commands: `.venv/bin/pytest` for the whole suite (strict xfails are known gaps), and
`.venv/bin/python -m kern check FILE.k` for a module.

## Where files go

- **`kern/`** ships: the toolchain, and nothing else. No experiments, harnesses or one-off
  scripts. It imports only from itself (plus z3 and the standard library).
- **`runtime/`** ships: the `kern-rt` crate.
- **`examples/`**: Kern programs people can read and run. Tests and frozen experiments read some
  of them by path, so don't rename them.
- **`tests/`**: the pytest suite, fast enough for every change. Small modules for tests go in
  `tests/probes/`.
- **`validation/`**: slower tools that check the toolchain itself (fuzzer, digests, Rust vs
  interpreter). Document their CLIs in `validation/README.md`.
- **`experiments/`**: measurements, each with its report in `docs/research/`. Nothing outside
  `experiments/` imports from it.
- **`build/`**: generated output (Rust crates, cargo target, ad hoc proof pages). Never commit it.

## Keep the docs current. This is a rule, not a suggestion.

**Update the docs in the same commit** whenever you change any of these:
- the syntax;
- a spec form;
- a handler or declaration;
- `check` output or proof-page wording;
- a CLI command or option;
- a public Python function.

Which file to update:
- **docs/REFERENCE.md:** the language. Keep it within 2,500 tokens; it's the measured
  "language tax" (docs/research/CHALLENGE.md, C2).
- **docs/GUIDE.md:** tools, files, API, errors.
- **CLAUDE.md's "Known limitations":** when a limit changes.

`tests/test_docs.py` enforces the mechanical part:
- every `kern` block must parse and check, and `kern clean` blocks must prove;
- every handler and language form must appear in REFERENCE.md;
- every CLI command and option must appear in GUIDE.md (the fuzzer's in validation/README.md);
- every API name GUIDE.md lists must exist;
- REFERENCE.md must stay within its token budget.

What it can't check is whether the prose is still *true*. Re-read the section you touched.

Mark code blocks `kern` (must parse and check), `kern clean` (must also prove) or `text`
(schematic).

## How to work here

- **Measure, don't assert.** When a claim can be tested cheaply, test it and report the
  number, including when it contradicts an earlier claim.
- **Pre-register:** write the pass conditions and the stop rule down before you run (see
  docs/research/SCALE.md and docs/research/CHALLENGE.md). Never edit a threshold after a run; add a dated note
  instead.
- **Report failures** with the same prominence as passes. A failed challenge is a result.
- **Don't change semantics to make a test pass.** If the host and the interpreter disagree,
  the host is wrong until shown otherwise. If a guarantee breaks, the code or the spec is
  wrong. Never hide the change.
- **Regression floors:** `check` output for examples/ledger_v3–v7 and ledger_large, and every
  existing build hash, stay identical unless the change is meant to alter them. When it is,
  say which and why.
- **Nothing correctness-critical lives in prose.** If something must be obeyed, it's a
  `need`, `keep`, `ensure` or `emit`; `.why` files are rationale only.

## Git, with other sessions in the same tree

- Several sessions may work in this checkout. Before you start, look for uncommitted work
  that isn't yours, and don't edit or commit it without asking its owner or the user. If you
  do commit it, commit it separately and attribute it.
- **Stage by path, never `git add -A`.** That's how another session's files once ended up in
  an unrelated commit.
- Commit only when the user asks, or when an agreed plan says to commit per step.

## Challenges and fresh agents

- A "fresh agent" is a headless `claude -p` run or a `general-purpose` subagent. It is
  **never a fork**: a fork inherits the design conversation.
- Give a fresh agent only the material its arm allows. Record the model, prompt and seed.
- Frozen material lives in `experiments/challenge/` and is committed before the run. Graders are scripts.
  Whoever plants bugs or writes answer keys doesn't grade.
