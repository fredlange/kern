# Kern language reference

A module is a `.k` file of definitions and declarations. Indent with two spaces. There are
no comments; rationale goes in a `.why` file. To check a module: `python -m kern check FILE.k`.

## Definitions

```text
NAME p:Type ...  [-> Int]        header, column 0
  SPEC CLAUSES                   what callers may rely on
body
  STATEMENTS                     how; callers never read this
```

- **Parameter types:** `Int` and `Account`.
- **`-> Int`** makes a function: pure (no state, no effects), total (no `need`), and every
  path sets `result`. Specs may call functions; nothing else.

### Statements

| form | meaning |
|---|---|
| `x = EXPR` | let. A name is never reused, not even in the other branch |
| `need COND else Ctor ARGS` | refuse with `Ctor ARGS` unless COND holds |
| `EXPR` | an effect: `ledger.debit a n`, `callee a b n`, `audit.emit (Ev a n)` |
| `if COND`, then an indented block, optionally `else` and a block | branches |
| `result = EXPR` | a function's result (once per path) |

### Expressions

- **Operands:** integers, names, and juxtaposition application `f a b`. Arguments are atoms,
  so parenthesise anything compound: `Insufficient (amount - have)`.
- **Arithmetic:** `+ - * /`. `*` needs a literal on one side; `/` needs a positive literal
  on the right (floor division).
- **Comparisons:** `= != < <= > >=`.
- **Logic:** `and` binds tighter than `or`; `distinct a b c`.
- **Events and failures:** Capitalised constructors, such as `Moved a b n`.

## Handlers: the only effects

| name | gives | spec may use |
|---|---|---|
| `ledger.balance a`, `ledger.owner a`, `ledger.total` | Int / Owner / Int | yes (reads) |
| `ledger.debit a n`, `ledger.credit a n` | change a balance | no |
| `clock.now` | the call's time: one value per call | yes, with no after-state |
| `audit.emit (Ev …)` | append to the audit log | no; describe it with `emit` |

**Module declarations** (column 0):

| declaration | gives you |
|---|---|
| `state NAME Int` or `state NAME Account` | a map to Int (default 0): `NAME.get k` (specs may read it), `NAME.set k v` |
| `invariant x:Type COND` | COND holds for every key, before and after every call. It may read state only at `x` |
| `channel NAME` | another output stream: `NAME.emit (Ev …)` |

**After-state:** in `ensure` only, write `ledger.balance' a` or `NAME.get' k`.

## Spec clauses

| clause | meaning |
|---|---|
| `need COND else Ctor ARGS` | runs before the body; the call refuses unless COND holds |
| `ok otherwise` | claim: if every `need` holds, the call succeeds. Body and callee refusals must follow from the needs, and refusing callees must say `ok otherwise` too |
| `keep EXPR` | same value before and after a successful call |
| `ensure COND [when C]` | must hold after success; `'` reads the after-state, and `when C` (about the before-state) makes it conditional |
| `emit [CHANNEL] Ctor ARGS [when C]` | the exact sequence the call appends to the channel (default `audit`). `_` matches any value. A channel with no `emit` clause is unconstrained |
| `need as F ARGS`, `emit as F ARGS [when C]` | F's spec needs or emits, with F's parameters replaced by ARGS. Only right when F runs first |
| `eg WORLD \| NAME ARGS -> OUTCOME [\| AFTER]` | an executed example |

A refusal undoes the whole call, so guarantees only have to hold on success. Effects
(`uses`), failures (`fails`) and written balances (`writes`) are inferred: never write them.

### `eg` lines

- **WORLD:** `A:500@u1` (account, balance, owner), `now:5`, and state such as `hold.7:100` or
  `limit.A:300`.
- **ARGS:** integers and account names.
- **OUTCOME:** `ok`, or `Ctor ints` (for example `Insufficient 50`).
- **AFTER:** only the items listed are compared, such as `A:400 hold.7:0`.

## What `check` prints

Every guarantee gets an icon:
- ✔ proven for all inputs;
- ✗ refuted, with an `eg` counterexample that replayed;
- `?` no proof (unconfirmed counterexample, or no answer);
- `!` proven, but relies on a callee that isn't;
- ◐ held in random tests only;
- ∅ the call can never succeed.

Each definition gets one status:

| status | means |
|---|---|
| All guarantees proven | clean |
| Broken guarantees | a ✗ or a failing `eg` |
| Never succeeds | ∅ |
| Depends on unproven code | `!` |
| Not proven: … / Tested only, not proven | `?` or ◐ |
| No guarantees stated | there's nothing to prove |
| Weaker than baseline | below the recorded `kern baseline` |
| Spec changed since baseline | a `need` was added, removed or changed |

`spec strength` is mutation testing of the body:
- **guarantees** = mutants the proofs and random tests catch, out of all mutants;
- **missed** lines name what no guarantee pins down.

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
