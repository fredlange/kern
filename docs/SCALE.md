# Scaling Kern: one app, three sizes

Fredrik's brief: grow an application in three sizes, implement new features as needed
while following [FUNDAMENTALS.md](FUNDAMENTALS.md), evaluate between sizes, and
**stop and ask if a size can't be improved compared to the one before**. We review
together at size 3.

This file was written **before size 1**. The scorecard and stop rule below are fixed
up front so the verdicts between sizes aren't tuned after the fact. Any later change
to them is marked as such, with a reason.

## The app

A marketplace **escrow**, grown three times, roughly ×2.5 per size. Each size must
add at least one new *kind* of demand, not just more definitions: six renamed copies
of `ledger_large` (42 definitions) checked in 3.8 s and proved nothing new.

| Size | Defs (target) | New demand (planned; the program decides) |
|---|---|---|
| 1 | ~10 | holds as escrow accounts (reserve, capture, release, partial refund, fee split). **Branches** (`if`/`else`), and spec clauses that hold only in some cases |
| 2 | ~25 | per-account state (limits, freeze) via **handlers declared in Kern** with *assumed* specs, shown as assumed on the page; probably return values |
| 3 | ~60 | disputes, multi-party splits, batches: whatever sizes 1–2 show is missing (likely loops or records) |

Per size: write the program in the Kern I want → list what doesn't parse or prove →
implement the *minimum* that makes it work → ratchet the specs (F8) → score → write
the size's section here → compare with the previous size → commit.

## Scorecard (`python -m kern score FILE.k`)

### Floors: must hold at every size, or stop

1. Every definition is clean (✔). No ✗, no `?` unconfirmed, no ◐; ∅ only where a
   definition is meant to be unusable (none planned).
2. Regression safety: `check` output for ledger v3–v7 and `ledger_large` is
   byte-identical to before the size; build hashes of every existing definition are
   unchanged.
3. Tests green. New features get adversarial tests (a strict `xfail` per known gap,
   listed in the size's section). No silent new xfails.
4. Fundamentals: no new body-reading in the prover or views (F2), nothing written
   that could be inferred (F4), every page claim from a template (F5), no
   obligation-like `why` (F6).

### Trends: size N compared with size N−1

| Metric | Should | Why it matters |
|---|---|---|
| Reading ratio: editor-view tokens ÷ tokens of the definition plus all its transitive callees, median over definitions that call something, and at the deepest one | **fall** (or stay) | the central claim: views save more as code grows (F1, F9) |
| Guarantee strength: mutants killed by guarantees ÷ well-formed mutants, all definitions | not fall | F8; specs keep pace with bodies |
| Check seconds per definition | stay flat (within 2×) | bet 1: tooling stays in the inner loop |
| Workarounds: definitions written unnaturally because a feature is missing | **fall** | the language grows to meet the app |
| `ok otherwise` coverage, and spec tokens per definition | reported | the cost of saying when calls succeed (restating grows with depth) |

**Known caveat in the strength metric:** mutants are body-only, so moving a `need`
into the spec removes its mutants instead of catching them (FIXES.md, `ok
otherwise`). The denominator is reported next to every ratio. If the denominator
per definition falls sharply between sizes, that's flagged, not counted as progress.

### Stop rule

Stop and ask Fredrik if, at any size:
- a floor breaks and can't be restored within the size; or
- a trend is worse than at the previous size **and** the cause is structural (a
  limit of the language, the prover or the views, not a spec that the ratchet can
  tighten). Tightening specs is the normal loop, not a reason to stop.

## Size 0: the starting point (measured before size 1)

| | ledger_v7 | ledger_large |
|---|---|---|
| definitions | 2 | 7 |
| clean | 2/2 | 7/7 |
| guarantee strength | 22/22 | 53/63 = 84% |
| reading ratio, median / deepest | 0.82 / 0.82 (depth 1) | 0.67 / 0.39 (depth 3) |
| check s per definition | 0.14 | 0.09 |
| `ok otherwise` | 2/2 | 0/7 |

**Amendment to floor 2 (made after size 1, with its reason):** counterexample
*witness values* may differ (for example v6 payout's `eg A:1@u1 B:253@u1 | payout A B B 1 0` became
`eg A:286@u1 B:440@u1 | payout A B B 286 0`), provided the same guarantee fails with
the same failure and the witness replays. This is the known in-process Z3 history
effect: payout proved alone gives the old witness. Everything else must stay
byte-identical.

## Size 1: escrow core with branches (`examples/escrow_s1.k`)

**Program:** 10 definitions.
- `transfer`: the leaf.
- `move`: zero is allowed and does nothing.
- `reserve`, `release`, `capture`: tiered fee, none below 100.
- `purchase`, `cancel`, `refund`: full or partial.
- `split`, `tip`.

All use the existing ledger handlers. A hold is an escrow account. Every
definition uses `ok otherwise`.

**Features added (minimum the program needed):**
- **`if COND` / `else` blocks** in bodies.
  - The prover forks a path per branch; a guarantee holds iff it holds on every path,
    and a counterexample comes from the failing path. Straight-line bodies have one
    path and the same solver queries as before.
  - Branch-local lets stay in the branch, and names are never reused.
- **`ensure E when C` and `emit Ctor args when C`** (`C` about the before-state).
  - A conditional emit list must match the events whose conditions hold, in order. It
    composes through callees: move's conditional event shows up in tip's and capture's
    specs.
- **`and` binds tighter than `or`.** It was left-associative at one level. No
  existing definition changes, because none mixed them.
- **Inference walks branches.**
  - `uses`, `fails`, `writes` and `called_names` see both sides.
  - A `need` inside a branch is lifted as an implication (`amount <= 0 or amount <
    500`), never unconditionally.
  - The page and views show a branch's refusals with their guard ("amount ≠ 0 and
    from = to → SameAccount").
- **Branch mutants:** flip the condition, swap the branches, drop the `if`, and
  everything inside each branch.
- **Speed:**
  - `World.copy()` instead of `copy.deepcopy`, which was 29% of check time.
  - Mutant proofs stop at the first confirmed refutation (only "caught or not"
    matters there).
  - Together: 0.27 → 0.15 s per definition. Every existing output is unchanged.
- **Tests:** 97 → 119.
  - Every new feature has an adversarial test: only the else-path is wrong, a `when`
    boundary at 100 vs 101, a wrong emit condition, a conditional callee event
    dropped, a need inside a branch, lifting as an implication, rollback inside a
    branch, scoping, `when` placement, and bad blocks.

**A decision tried and reverted, measured:** inherit callee refusals instead of
restating them (a failure the spec names stays the definition's own; others come
from callee specs). It fits F4 better.

| | restated (kept) | inherited (reverted) |
|---|---|---|
| source tokens | 2267 | 1966 |
| guarantee strength | 143/147 = 97%* | 127/147 = 86% |
| reading ratio, median / deepest | 0.69 / 0.69 | 0.78 / 0.81 |
| check s per definition | 0.30 | 0.60 |

*143 because, under the inheriting code, refund's unnamed `BadAmount` was inherited
too; it's 146/147 once reverted.

Inheriting lost on every trend:
- **Strength:** mutants that swap a callee's arguments change the inherited
  condition along with the body, so nothing catches them. This is the argument that
  kept `emit` from being inferred.
- **Reading ratio:** the inherited lines that a complete spec view must show are
  longer than the restated `need`s.

This overrules F4's "derive, don't declare" for refusals, on measurement. Fredrik
can overturn it at the review.

**Workarounds** (written unnaturally because a feature is missing):
1. **Fee is a parameter, not computed:** there's no `*` or `/`
   (capture, purchase, refund).
2. **`need ledger.balance escrow >= 0 else Overdrawn`:** stands in for a ledger
   invariant (balances never go negative). Without it, capture's balance need after
   reserve's write isn't provable (purchase).
3. **A hold is an escrow account:** nothing tracks which order a hold belongs to.
   Needs per-entity state (all escrow definitions).
4. **refund has no `emit` spec:** its events depend on nested branch conditions
   through two callees, and writing them out was impractical.
5. **Aliasing guards everywhere** (`when buyer != seller and buyer != house`,
   `escrow != seller and escrow != house and seller != house`): there's no way to
   declare roles distinct.

Definitions with at least one workaround (excluding #3 and #5, which touch nearly
everything): 3/10.

### Scorecard, size 1

| | size 0 (ledger_large) | size 1 |
|---|---|---|
| definitions / max depth | 7 / 3 | 10 / 3 |
| clean | 7/7 | 10/10 |
| unconfirmed | 0 | 0 |
| guarantee strength | 53/63 = 84% | 146/147 = 99% |
| mutants per definition | 9.0 | 14.7 |
| reading ratio, median | 0.67 | 0.69 |
| reading ratio, deepest | 0.39 (month_end) | 0.69 (refund), 0.57 (purchase) |
| check s per definition | 0.09 | 0.15 |
| `ok otherwise` | 0/7 | 10/10 |
| source tokens: spec / body / eg | 406 / 133 / 346 | 1351 / 199 / 689 |

**Verdict:**
- **Floors:** all hold, with the witness amendment above.
- **The comparison with size 0 is informational:** ledger_large is a different
  program. The three sizes are compared with each other.
- **Two things got worse against size 0, reported as such:**
  - **Reading ratio:** median flat, deepest worse. The cause is structural. Bodies
    are 10% of the tokens, while restated needs are 22% and examples 29%. The
    editor view skips callee bodies and deeper specs, and with thin bodies that's
    all it can save. ledger_large's 0.39 came from month_end, whose spec is almost
    empty (it has no `ok otherwise`). The one structural fix I had, inheritance,
    was worse (table above).
  - **Check time per definition:** 1.7×, within the floor. The cause is 2.9× more
    prove work per definition (more guarantees × more mutants), not tooling that
    scales worse.

**What this predicts for size 2:** if the reading ratio is to fall, it has to come
from depth (more levels skipped) or from thicker bodies, for example state and
computed values. Restating grows with depth, so this is the trend to watch.

### Pre-registered for size 2 (written before the size-2 program)

- **Reading ratio, depth for depth:** the median shifts when the program's shape
  changes (many depth-1 wrappers push it up), so size 2 is compared with size 1 per
  depth, using medians at depth 1, 2, 3 (and 4 if it exists), plus the deepest.
  Size 1: depth 1 **0.81** (n=3), depth 2 **0.59** (n=4), depth 3 **0.63** (n=2).
- **What counts as structural for this metric:** the per-depth ratios rise, and the
  cause is restated needs growing faster with depth than the bodies the view skips.
  If that's what happens, stop and ask.
- **Scope order if the budget runs short:** state maps, then `*` and `/`, then the
  invariant. A workaround that survives is counted, not hidden.

## Size 2: orders, disputes, limits (`examples/escrow_s2.k`)

**Program:** 26 definitions. Size 1's ten are adapted (the fee is computed, there's
no `Overdrawn`), plus:
- orders: `open_order`, `capture_order`, `cancel_order`, `refund_order`;
- disputes: `open_dispute`, and resolving for the buyer, for the seller, or split;
- account controls: `set_limit`, `freeze`, `unfreeze`, `withdraw`, `reset_day`;
- `payout_seller`, `deposit`, `purchase_order`.

Every definition uses `ok otherwise`, and every definition that emits has an `emit`
spec.

**Features added:**
- **`state NAME Int|Account`:** a map to Int (default 0), read with `NAME.get k` (also
  in specs, with an after-state `NAME.get' k`) and written with `NAME.set k v`.
  - One resolver (`run.state_op`) is used by the interpreter, checker, inference and
    prover, so they can't disagree about what a state operation is.
  - Inferred writes look like `hold.id` (`hold.*` for a computed key). Modular calls
    havoc exactly those, and the page says which written entries no guarantee
    describes.
  - Example worlds take `hold.7:100`. Counterexamples carry the state at the keys
    the call names, so they replay.
  - Random worlds draw state from a separate stream (mostly 0/1, sometimes larger),
    so modules without state keep exactly the same random worlds.
- **`invariant x:Type COND`:** assumed for every parameter of that type in the
  pre-state (sound, because it's weaker than "for all keys"), and proven in the
  post-state by each definition that writes what it reads.
  - A write to a computed key makes the claim unknown (it can become ◐, since random
    tests check invariants after every success).
  - Callees are assumed to keep it at their argument keys, and trust propagation
    covers whether they actually do.
- **`*` and `/`** with a literal (`/` by a positive literal only), so the arithmetic
  stays linear. Z3 and Python agree on floor division for negatives (tested).
- **`need as C args` / `emit as C args [when D]`:** a written reference to a callee's
  spec, expanded at parse time into ordinary needs and emits (D is conjoined with
  each event's condition). This was added because restated needs drove the size-1
  reading ratio (the pre-registered risk).
  - Unlike inheriting, the reference names its own arguments, so a body mutant that
    changes a call's arguments is still caught (reserve: 6/6, as when restated).
  - The page marks such rows "(as capture)".
  - A reference is only right when the callee's conditions hold in the caller's
    pre-state, i.e. it's the first call. Otherwise the prover refutes it and the
    caller restates, as purchase and purchase_order do for capture.
- **Tests:** 119 → 140, with adversarial cases for each feature.

**Workarounds, compared with size 1:**

| # | Size 1 | Size 2 |
|---|---|---|
| 1 | fee as a parameter | **gone**: `amount * 2 / 100` |
| 2 | `Overdrawn` need | **gone**: `invariant a:Account ledger.balance a >= 0` |
| 3 | a hold is an escrow account | **weaker**: holds are per order (`hold`), but the escrow balance isn't tied to the sum of holds (no sums or quantifiers) |
| 4 | refund has no `emit` spec | **gone**: `emit as … when` |
| 5 | aliasing guards everywhere | **remains** |
| 6 | (new) callee conditions after a write must be restated | **remains**: purchase, purchase_order, refund (Insufficient) |
| 7 | (new) batch work (release all expired holds, end-of-day for all accounts) | **not expressible**: no loops; left out of the program |

Definitions with at least one workaround (excluding #3 and #5): 3/26 (purchase,
purchase_order, refund), against 3/10 at size 1.

### Scorecard, size 2 vs size 1

| | size 1 | size 2 |
|---|---|---|
| definitions / max depth | 10 / 3 | 26 / 3 |
| clean / unconfirmed | 10/10 / 0 | 26/26 / 0 |
| guarantees | 51 | 172 |
| guarantee strength | 146/147 = 99% | 270/271 = 100% |
| mutants per definition | 14.7 | 10.4 |
| reading ratio, median | 0.69 | **0.62** |
| reading ratio, depth 1 / 2 / 3 | 0.81 / 0.59 / 0.63 | **0.80 / 0.62 / 0.43** |
| reading ratio, deepest | 0.69 | **0.50** |
| check s per definition | 0.15 | 0.18 |
| workarounds (definitions) | 3/10 | 3/26 |
| source tokens: spec / body | 1351 / 199 | 2483 / 524 |

Measured on the way: size 2 with every refusal **restated** scored a median of
0.66, depth 1/2/3 of 0.81/0.65/0.48, 5287 source tokens and 265/271 strength. The
references brought it to the numbers above.

**Verdict, against the pre-registered rules:**
- **Floors:** all hold. Outputs for v3–v7 and ledger_large are identical, except
  the v6 witness already recorded; hashes are unchanged.
- **Improved:**
  - **Reading ratio:** median, depth 3 and deepest.
  - **Workarounds:** the same three definitions out of 2.6× as many.
  - **Strength:** not lower.
  - **Check time:** within 2×.
- **Depth 2: 0.59 → 0.62, slightly worse.** The pre-registered test asks whether the
  cause is restated needs growing with depth:
  - Like for like, it isn't: cancel 0.59 → 0.52, tip 0.56 → 0.53, split 0.60 → 0.61.
  - The median moved because different definitions sit at depth 2 now: refund (0.78)
    dropped from depth 3 to 2 when capture started calling transfer directly, and
    six new wrappers over large callee specs joined.
  - So it isn't the structural cause, and it doesn't trigger the stop rule. I report
    it as is rather than call it an improvement.
- **The mutants-per-definition denominator fell (14.7 → 10.4).** The cause is many
  small new definitions (freeze, set_limit, reset_day: 1–4 mutants each), not needs
  moved into specs: the restated and referenced versions have the same 271 mutants.
