# Kern language reference

A module is a `.k` file of definitions and declarations. Indent with two spaces. There are
no comments; rationale goes in a `.why` file. To check a module: `kern check FILE.k`.

## Definitions

```text
NAME p:Type ...  [-> Int]        header, column 0
  SPEC CLAUSES                   what callers may rely on
body
  STATEMENTS                     how; callers never read this
```

- **Parameter types:** `Int`, `Account`, `Text` and declared records. A record parameter `p:Point` is read by field,
  `p.x`, or passed whole to a definition taking a Point (`move p`); build one only as a call argument,
  `move (Point a b)`. In `eg` lines its fields are separate arguments, in order. A field may be a record
  (`record Line a:Point b:Point`): read `l.a.x`, pass `l.a` as a Point. `p = q` and `p != q` compare every field.
- **`-> Int`**: the definition returns a value; every path sets `result`, and its spec calls it `result`. One
  that only reads (state, the clock) and never refuses is a *function*: specs and expressions may call it. One that
  changes state or refuses is called only as a whole statement, `loan = borrow book` (or `borrow book`, dropping
  the value). Which one it is, is inferred.

### Statements

| form | meaning |
|---|---|
| `x = EXPR` | let. A name is never reused, not even in the other branch |
| `need COND else Ctor ARGS` | refuse with `Ctor ARGS` unless COND holds |
| `EXPR` | an effect: `ledger.debit a n`, `callee a b n`, `audit.emit (Ev a n)` |
| `if COND`, then an indented block, optionally `else` and a block | branches |
| `match E`, then arms `PAT -> STMT` (or `PAT ->` and a block) | the `if E = PAT … else …` chain; PAT is an integer, a constant, a Text or a last `_`. n arms are n paths |
| `for k from LO to HI`, then an indented block | a loop: k = LO … HI−1 (bounds evaluated once). It starts with `invariant CLAUSE` lines; loops nest |
| `result = EXPR` | the value a `-> Int` definition returns (once per path) |

### Expressions

- **Operands:** integers (literals fit in i64), Text literals `"…"` (escape `\"` and `\\`), names, and
  juxtaposition application `f a b`. Arguments are atoms,
  so parenthesise anything compound: `Insufficient (amount - have)`.
- **Arithmetic:** `+ - * /`. `/` needs a positive literal on the right (floor division). `*` of two
  variables is allowed, but the prover may give no answer (`?`) where it is nonlinear.
- **Comparisons:** `= != < <= > >=` (Text: only `=` and `!=`).
- **Text:** `length t` is its number of characters; `a + b` joins two Texts.
- **Logic:** `and` binds tighter than `or`; `distinct a b c`.
- **Sums:** `sum j from A to B of E [where C]` adds E for each j in [A, B) where C holds (bounds are
  atoms; E can't call definitions; no sum inside a sum). E runs to the next comparison; `where C` runs to the
  end, so parenthesise a filtered sum that isn't last.
- **Events and failures:** Capitalised constructors, such as `Moved a b n`.

## Handlers: the only effects

| name | gives | spec may use |
|---|---|---|
| `ledger.balance a`, `ledger.owner a`, `ledger.total` | Int / Owner / Int | yes (reads) |
| `ledger.debit a n`, `ledger.credit a n` | change a balance | no |
| `clock.now` | the call's time | yes: in a spec, the time the call started (no after-state) |
| `clock.tick` | moves the call's time on by exactly 1 (a later instant in the same call) | no |
| `audit.emit (Ev …)` | append to the audit log | no; describe it with `emit` |

**Module declarations** (column 0):

| declaration | gives you |
|---|---|
| `state NAME Int` or `state NAME Account` | a map to Int (default 0): `NAME.get k` (specs may read it), `NAME.set k v` |
| `state NAME Int -> Text` (or `Account -> Text`) | a map to Text (default `""`) |
| `state NAME Int -> R` (R a record of Int and Text fields) | one map per field, `NAME.f`: `NAME.f.get k`, `NAME.f.set k v`. Whole: `NAME.get k` where an R goes (an argument, `=`), `NAME.set k r` (a statement), `keep NAME` |
| `invariant x:Type COND` | COND holds for every key, before and after every call. It may read state only at `x` |
| `record Name f:Type …` | a record type: Int, Account or Text fields. Views add its `record` line |
| `const NAME N` | a named integer (N ≥ 0), usable wherever a literal is (`amount * rate` stays linear). Views add a `const NAME N` line |
| `channel NAME` | another output stream: `NAME.emit (Ev …)` |
| `handler a.b p:Int -> Int`, then `need` and `ensure` lines (no body) | an observation inside a call, such as `rand.below n`. Its spec is *assumed*: it may read only its arguments and `result`. Its `need`s are preconditions every call must meet. The host provides the value and checks the spec at runtime |

**States:** an unprimed read is the state before the call. A primed read (`ledger.balance' a`,
`NAME.get' k`) is the state the clause is about: after the call in `ensure`, at the start of
each iteration in a loop `invariant`.

## Packages and imports

A directory with a `kern.pkg` file is a package; each `.k` file in it is a module named by its
path (`money/ledger.k` is `money.ledger`). `import money.ledger` (column 0) lets a file use that
module's definitions, state and handlers, unqualified. A file uses only what it declares or
imports; names are unique across the package. Checking any file checks the whole package. A `use ../money`
line in `kern.pkg` makes a sibling package's modules importable as `money.ledger` (transitively).

## Spec clauses

| clause | meaning |
|---|---|
| `need COND else Ctor ARGS` | runs before the body; the call refuses unless COND holds |
| `decreases E` | the termination measure a recursive definition needs: an Int over the parameters, ≥ 0 and smaller at every recursive call (proven per call) |
| `ok otherwise` | claim: if every `need` holds, the call succeeds. Body and callee refusals must follow from the needs, and refusing callees must say `ok otherwise` too |
| `keep EXPR` | same value before and after a successful call |
| `keep X [except A to B]` | state X is unchanged, or unchanged at every key outside [A, B) (Int keys; bounds atoms): a stated frame |
| `ensure COND [when C] [for j from A to B]` | must hold after success; `when C` (about the before-state) makes it conditional, and `for j …` makes it hold for every j in [A, B). Bounds are atoms |
| `emit [CHANNEL] Ctor ARGS [when C]` | the exact sequence the call appends to the channel (default `audit`). `_` matches any value. A channel with no `emit` clause is unconstrained |
| `need as F ARGS`, `emit as F ARGS [when C]` | F's spec needs or emits, with F's parameters replaced by ARGS. Only right when F runs first |
| `eg WORLD \| NAME ARGS -> OUTCOME [\| AFTER [\| EVENTS]]` | an executed example |

A refusal undoes the whole call, so guarantees only have to hold on success. Effects
(`uses`), failures (`fails`) and written balances (`writes`) are inferred: never write them.

### `eg` lines

- **WORLD:** `A:500@u1` (account, balance, owner), `now:5`, and state such as `hold.7:100`,
  `limit.A:300`, `title.1:"a b"` or a stored record's field, `order.qty.7:2`.
- **ARGS:** integers, account names and Text literals.
- **OUTCOME:** `ok`, `ok N` (also asserts the returned value), or `Ctor ints` (for example `Insufficient 50`).
- **AFTER:** only the items listed are compared, such as `A:400 hold.7:0`.
- **EVENTS:** `Ev args, [channel] Ev args`: the exact events appended to `audit` (an empty section: none) and
  to each channel named, such as `| | Moved A B 100 0`.

## What `check` prints

Every guarantee gets an icon:
- ✔ proven for all inputs;
- ✗ refuted, with an `eg` counterexample that replayed;
- `?` no proof (unconfirmed counterexample, or no answer);
- `!` proven, but relies on a callee that isn't;
- ◐ held in random tests only;
- ○ a module invariant with no proof, checked at run time after every call (`kern host`, `kern build`);
- ∅ the call can never succeed;
- ⊙ proven, assuming a handler's declared spec.

Each definition gets one status:

| status | means |
|---|---|
| All guarantees proven | clean |
| Broken guarantees | a ✗ or a failing `eg` |
| Never succeeds | ∅ |
| Depends on unproven code | `!` |
| Not proven: … / Tested only, not proven | `?` or ◐ |
| Checked at run time only, not proven | ○ |
| No guarantees stated | there's nothing to prove |
| Weaker than baseline | below the recorded `kern baseline` |
| Spec changed since baseline | a `need` was added, removed or changed |

`spec strength` is mutation testing of the body:
- **guarantees** = mutants the proofs and random tests catch, out of all mutants;
- **missed** lines name what no guarantee pins down;
- **spec refusals**: each spec `need` dropped or its comparison flipped; an `unpinned` one wants an `eg` at its boundary.

## Example 1: a guarded transfer and a zero-tolerant caller

```kern clean
transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit Moved from to amount clock.now
  eg A:500@u1 B:0@u2 | transfer A B 100 -> ok | A:400 B:100
  eg A:50@u1 B:0@u2 | transfer A B 100 -> Insufficient 50
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (Moved from to amount clock.now)

move from:Account to:Account amount:Int
  need amount >= 0 else BadAmount
  need amount = 0 or from != to else SameAccount
  need amount = 0 or ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' to = ledger.balance to + amount
  emit as transfer from to amount when amount > 0
  eg A:500@u1 B:0@u2 | move A B 0 -> ok | A:500 B:0
body
  if amount > 0
    transfer from to amount
```

## Example 2: state, an invariant and a function

```kern clean
state hold Int

invariant o:Int hold.get o >= 0

fee_for amount:Int -> Int
  ensure result = 0 when amount < 100
  ensure result = amount * 2 / 100 when amount >= 100
body
  if amount < 100
    result = 0
  else
    result = amount * 2 / 100

open_order id:Int amount:Int
  need hold.get id = 0 else OrderExists
  need amount > 0 else BadAmount
  ok otherwise
  keep ledger.total
  ensure hold.get' id = amount - fee_for amount
  eg | open_order 7 250 -> ok | hold.7:245
  eg hold.7:3 | open_order 7 250 -> OrderExists
body
  hold.set id (amount - fee_for amount)
```
