# Kern — design fundamentals

These are the principles the prototype is judged against. `docs/research/AUDIT.md` records
how well the current code meets each one, with tests in `kern/tests/fundamentals_*.rs`.

## The nine fundamentals

### 1. Optimise for the reader, not the writer
Generation is cheap; context, memory and trust are scarce.

*Rationale:* an agent can produce code at almost no cost. What runs out is the
context window, what it can remember between tasks, and the human's attention. Every
design choice is judged by what it costs whoever reads the result next.

### 2. Verification licenses not reading
An agent may skip a body only if the interface is machine-checked. The prover
follows the same rule: callers are verified using callee specs only.

*Rationale:* skipping a body is only safe when what you read instead is guaranteed
to describe it. If the prover itself needed callee bodies, its proofs would not
justify an agent skipping them. Minimal reading and the proof page are therefore
the same mechanism.

### 3. The spec is the primary artifact
Humans own what and why; agents own how. Review is of guarantees, not diffs.

*Rationale:* a human can't keep up with reviewing agent-written diffs line by line,
but can judge whether a list of guarantees is the right one. The body is the
agent's business, provided it provably meets the spec.

### 4. Derive, don't declare
Anything inferable (effects, failures, interface) is computed, never written.

*Rationale:* a declaration the compiler could infer is ceremony for the writer
and a possible lie for the reader. Computing it removes both the tokens and the
drift.

### 5. Evidence over assertion
Every human-facing claim traces to a check and states its strength. Claims are
rendered by templates, never by a model, and weakness propagates.

*Rationale:* a model narrating code faithfully narrates its bugs, loses
information, and varies between runs. Fixed templates over verifier results
can't say more than the checks established. A guarantee that relies on a broken
callee must say so.

### 6. Separate knowledge by role
Obligations are compiler-enforced. Rationale is lazy, hash-anchored prose.
Nothing that must be obeyed lives only in prose.

*Rationale:* folding comments away is only safe if no hidden constraint lives in
them. So constraints become `need`/`keep`/`ensure`/`emit`, which are checked, and
the remaining prose (`why`) can be loaded lazily. It is anchored to the code's
hash so it never rots silently.

### 7. Closed world of effects
Everything the code touches goes through declared handlers, so effects are
inferable, replayable and enforceable.

*Rationale:* effect inference (F4) is only exact if there is no side door.
Handlers also give deterministic replay and simulation, which the runtime roadmap
step builds on.

### 8. The ratchet loop
Counterexamples become examples, and surviving mutants become guarantees. The spec
only gets stronger, and its strength is measured.

*Rationale:* specs written by agents will start weak. The loop turns each
weakness the tooling finds into a stronger spec, and mutation testing measures
how much of the body's behaviour the spec actually pins down.

### 9. Tokens shape syntax, but they aren't the main lever
Use single-token words and no ceremony, but what gets loaded matters more than
density.

*Rationale:* syntax is measured in tokens (one-token English keywords; Unicode
symbols cost 2–3 tokens), but the big savings come from *not loading* things:
callee bodies and folded rationale. See F9 in the audit for the measured scaling.

## The three bets

The approach works only if all three hold. None of them is established yet.

1. **Verification tooling is good enough.** SMT-backed modular verification can
   prove the guarantees that matter for real code (with frames, loops, records and
   effects) fast enough and with few enough false alarms to sit in an agent's
   inner loop.
2. **Agents can write strong specs.** With the ratchet's feedback (counterexamples,
   surviving mutants), agents write specs strong enough that "the spec is proven" is
   close to "the code is right".
3. **Humans can judge guarantees from a proof page.** A person who never reads the
   body can tell from template-rendered guarantees, tiers and spec strength whether
   these are the *right* guarantees, and notice when an important one is missing.
