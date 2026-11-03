"""C6 generator (frozen before measuring): synthetic, correct Kern modules.

    gen(d, k, e, n, state) -> module source

n definitions in chains of depth d: f{c}_1 is a transfer-like leaf, f{c}_j calls f{c}_{j-1}.
Every body has k sequential `if amount > T` blocks (2^k paths), every definition e own emits
(the first min(k, e) inside the ifs, so conditional), and callers restate refusals with
`need as` and events with `emit as`, so a definition at depth j has about j*e events in its
spec (the growth seen in checkout2). With state on, every level increments `cnt.get a` under
`invariant a:Account cnt.get a >= 0`.
"""


def gen(d, k, e, n, state=False):
    out = []
    if state:
        out += ["state cnt Account", "invariant a:Account cnt.get a >= 0", ""]
    chains = [list(range(1, d + 1))] * (n // d) + ([list(range(1, n % d + 1))] if n % d else [])
    for c, levels in enumerate(chains):
        for j in levels:
            out += definition(c, j, k, e, state) + [""]
    return "\n".join(out)


def definition(c, j, k, e, state):
    name, callee = f"f{c}_{j}", f"f{c}_{j - 1}"
    cond = min(k, e)
    lines = [f"{name} a:Account b:Account amount:Int"]
    if j == 1:
        lines += ["  need amount > 0 else BadAmount", "  need a != b else SameAccount",
                  "  need ledger.balance a >= amount else Insufficient (amount - ledger.balance a)"]
    else:
        lines += [f"  need as {callee} a b amount"]
    lines += ["  ok otherwise", "  keep ledger.total",
              "  ensure ledger.balance' a = ledger.balance a - amount",
              "  ensure ledger.balance' b = ledger.balance b + amount"]
    if state:
        lines += [f"  ensure cnt.get' a = cnt.get a + {j}"]
    lines += [f"  emit E{i} a amount clock.now when amount > {10 * (i + 1)}" for i in range(cond)]
    if j > 1:
        lines += [f"  emit as {callee} a b amount"]
    lines += [f"  emit F{i} a clock.now" for i in range(cond, e)]
    lines += [f"  eg A:500@u1 B:0@u2 | {name} A B 100 -> ok | A:400 B:100"]
    lines += ["body"]
    if state:
        lines += ["  cnt.set a (cnt.get a + 1)"]
    for i in range(k):
        lines += [f"  if amount > {10 * (i + 1)}"]
        lines += [f"    audit.emit (E{i} a amount clock.now)" if i < e else "    ledger.credit b 0"]
    if j == 1:
        lines += ["  ledger.debit a amount", "  ledger.credit b amount"]
    else:
        lines += [f"  {callee} a b amount"]
    lines += [f"  audit.emit (F{i} a clock.now)" for i in range(cond, e)]
    return lines
