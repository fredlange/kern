"""Examples, property tests and mutation testing."""
from __future__ import annotations

import copy
import dataclasses
import random

from .prove import Prover
from .run import Interp, KernError, World
from .syntax import code, stmt_code, with_def
from .wf import problems


def run_examples(defs, d):
    interp, out = Interp(defs), []
    for ex in d.examples:
        try:
            w = World.from_example(ex.world, ex.now, ex.maps)
            outcome, after = interp.run(ex.call[0], ex.call[1], w)
            ok = (outcome[0] == ex.outcome[0] and (ex.outcome[0] == "ok" or list(outcome[1]) == ex.outcome[1])
                  and all(after.bal[a] == b for a, b in ex.after.items())
                  and all(after.maps.get(x, {}).get(k, 0) == v for x, kv in ex.after_maps.items()
                          for k, v in kv.items()))
            got = "ok" if outcome == ("ok",) else " ".join([outcome[0], *map(str, outcome[1])])
            out.append({"src": ex.src, "pass": ok, "got": got})
        except (KernError, KeyError, TypeError) as e:
            out.append({"src": ex.src, "pass": False, "got": f"error: {e}"})
    return out


def _rand_args(d, accts, rnd):
    args = []
    for _, t in d.params:
        if t == "Account":
            args.append(rnd.choice(accts))
        else:
            args.append(rnd.choice([rnd.randint(-5, 5), rnd.randint(0, 200), rnd.randint(0, 1200)]))
    return args


def proptest(defs, d, n=500, seed=1):
    rnd, interp = random.Random(seed), Interp(defs)
    times = random.Random(seed + 1_000_003)     # separate stream: adding time must not reshuffle worlds
    per_state = {}                              # ... nor adding declared state
    stats = {"runs": n, "succeeded": 0, "failures": {}, "violations": []}
    from .infer import relevant_states, uses
    touched = uses(defs, d)
    states_used = relevant_states(defs, d)     # the state the call or its spec can see
    checks_invariants = any(e.endswith(".write") or e == "ledger.write" for e in touched)
    for _ in range(n):
        accts = ["A", "B", "C"]
        w = World({a: rnd.randint(0, 1000) for a in accts}, {a: rnd.choice(["u1", "u2"]) for a in accts},
                  times.randint(0, 100))
        for x, key_type in states_used.items():
            keys = accts if key_type == "Account" else range(-5, 6)   # Int args hit -5..5 a third of the time
            # mostly 0 or 1 (flags, empty slots), sometimes a larger value; one stream per state, so
            # which states are drawn does not change the values of the others
            r = per_state.setdefault(x, random.Random(f"{seed}:{x}"))
            w.maps[x] = {k: r.choice([0, 0, 1, r.randint(0, 300)]) for k in keys}
        args = _rand_args(d, accts, rnd)
        before = w.copy()
        try:
            outcome, after = interp.run(d.name, args, w)
        except (KernError, KeyError, TypeError) as e:
            stats["violations"].append({"error": str(e), "args": args})
            continue
        if outcome == ("ok",):
            stats["succeeded"] += 1
            broken = [("invariant", text) for text, _ in interp.broken_invariants(after)] \
                if checks_invariants and not interp.broken_invariants(before) else []
            for kind, c in interp.holds(d, args, before, after) + broken:
                if len(stats["violations"]) < 3:
                    world = " ".join(f"{a}:{before.bal[a]}@{before.own[a]}" for a in accts) + f" now:{before.clock}"
                    world += "".join(f" {x}.{k}:{v}" for x, kv in before.maps.items() for k, v in kv.items() if v)
                    stats["violations"].append({"clause": f"{kind} {c}",
                                                "eg": f"eg {world} | {d.name} {' '.join(map(str, args))}"})
        else:
            stats["failures"][outcome[0]] = stats["failures"].get(outcome[0], 0) + 1
            if interp.unstated_refusal(d, args, before, outcome) and len(stats["violations"]) < 3:
                world = " ".join(f"{a}:{before.bal[a]}@{before.own[a]}" for a in accts) + f" now:{before.clock}"
                got = " ".join([outcome[0], *map(str, outcome[1])])
                stats["violations"].append({"clause": "ok otherwise",
                                            "eg": f"eg {world} | {d.name} {' '.join(map(str, args))} -> {got}"})
    return stats


# ---------- mutation ----------

SWAP = {"+": "-", "-": "+", "*": "/", "/": "*", ">=": ">", ">": ">=", "<=": "<", "<": "<=", "=": "!=", "!=": "="}


def _expr_mutants(e, params):
    k = e[0]
    if k == "bin" and e[1] in SWAP:
        yield f"`{e[1]}` → `{SWAP[e[1]]}`", ("bin", SWAP[e[1]], e[2], e[3])
    if k == "bin":
        for desc, m in _expr_mutants(e[2], params):
            yield desc, ("bin", e[1], m, e[3])
        for desc, m in _expr_mutants(e[3], params):
            yield desc, ("bin", e[1], e[2], m)
    if k == "app":
        name = e[1][0]
        if name in ("ledger.debit", "ledger.credit"):
            other = "ledger.credit" if name == "ledger.debit" else "ledger.debit"
            yield f"`{name}` → `{other}`", ("app", (other, e[1][1]), e[2])
        for i, a in enumerate(e[2]):
            for desc, m in _expr_mutants(a, params):
                yield desc, ("app", e[1], e[2][:i] + [m] + e[2][i + 1:])
    if k == "name" and e[1] in params:
        for p in params[e[1]]:
            yield f"`{e[1]}` → `{p}`", ("name", p, e[2])
    if k == "int":
        yield f"`{e[1]}` → `{e[1] + 1}`", ("int", e[1] + 1)
    if k == "name" and e[1] == "clock.now":
        yield "`clock.now` → `0`", ("int", 0)


def mutants(d):
    by_type = {}
    for p, t in d.params:
        by_type.setdefault(t, []).append(p)
    same_type = {p: [q for q in by_type[t] if q != p] for p, t in d.params}
    yield from _block_mutants(d.body, same_type)


def _block_mutants(body, same_type):
    """Every mutant of a statement list, recursing into `if` blocks. Straight-line bodies get exactly
    the mutants they always had, in the same order."""
    for i, s in enumerate(body):
        put = lambda new: body[:i] + [new] + body[i + 1:]
        yield f"drop `{stmt_code(s)}`", body[:i] + body[i + 1:]
        if s[0] == "for":                        # bounds +-1, the key off by one, an emptied body
            var, lo, hi, invs, inner = s[1:]
            one = ("int", 1)
            for desc, m in ((f"`{code(lo)}` → `{code(lo)} + 1`", ("bin", "+", lo, one)),
                            (f"`{code(lo)}` → `{code(lo)} - 1`", ("bin", "-", lo, one))):
                yield f"{desc} in `{stmt_code(s)}`", put(("for", var, m, hi, invs, inner))
            for desc, m in ((f"`{code(hi)}` → `{code(hi)} + 1`", ("bin", "+", hi, one)),
                            (f"`{code(hi)}` → `{code(hi)} - 1`", ("bin", "-", hi, one))):
                yield f"{desc} in `{stmt_code(s)}`", put(("for", var, lo, m, invs, inner))
            shifted = _shift_var(inner, var)
            if shifted != inner:
                yield f"`{var}` → `{var} + 1` in the body of `{stmt_code(s)}`", put(("for", var, lo, hi, invs, shifted))
            yield f"empty the body of `{stmt_code(s)}`", put(("for", var, lo, hi, invs, []))
            for desc, m in _block_mutants(inner, same_type):
                yield desc, put(("for", var, lo, hi, invs, m))
            continue
        if s[0] == "if":
            for desc, m in _expr_mutants(s[1], same_type):
                yield f"{desc} in `{stmt_code(s)}`", put(("if", m, s[2], s[3]))
            yield f"swap the branches of `{stmt_code(s)}`", put(("if", s[1], s[3], s[2]))
            for desc, m in _block_mutants(s[2], same_type):
                yield desc, put(("if", s[1], m, s[3]))
            for desc, m in _block_mutants(s[3], same_type):
                yield desc, put(("if", s[1], s[2], m))
            continue
        exprs = {"let": [2], "need": [1], "do": [1]}[s[0]]
        for j in exprs:
            for desc, m in _expr_mutants(s[j], same_type):
                new = list(s)
                new[j] = m
                yield f"{desc} in `{stmt_code(s)}`", put(tuple(new))
        if s[0] == "need":                       # the failure payload, e.g. Insufficient (amount - have)
            for k, a in enumerate(s[3]):
                for desc, m in _expr_mutants(a, same_type):
                    new = list(s)
                    new[3] = s[3][:k] + [m] + s[3][k + 1:]
                    yield f"{desc} in payload of `{stmt_code(s)}`", put(tuple(new))


def _shift_var(stmts, var):
    """Every use of the loop variable in a body as `var + 1` (the classic off-by-one in a key)."""
    def sub(e):
        if e == ("name", var, False):
            return ("bin", "+", e, ("int", 1))
        if e[0] == "bin":
            return ("bin", e[1], sub(e[2]), sub(e[3]))
        if e[0] == "app":
            return ("app", e[1], [sub(a) for a in e[2]])
        return e
    out = []
    for s in stmts:
        if s[0] == "let":
            out.append(("let", s[1], sub(s[2])))
        elif s[0] == "need":
            out.append(("need", sub(s[1]), s[2], [sub(a) for a in s[3]]))
        elif s[0] == "if":
            out.append(("if", sub(s[1]), _shift_var(s[2], var), _shift_var(s[3], var)))
        elif s[0] == "do":
            out.append(("do", sub(s[1])))
        else:
            out.append(s)
    return out


def mutation_score(defs, d, n_props=150):
    """Run every layer on every well-formed mutant, so the score can say *what* catches bugs.

    Guarantee strength = mutants caught by the guarantees (prover refutation confirmed in the
    interpreter, or a random test violating a guarantee). Examples are reported separately:
    they pin behaviour, but they are not the proven part of the spec."""
    rows, ill_formed, errors = [], [], []
    for desc, body in mutants(d):
        md = dataclasses.replace(d, body=body)
        mdefs = with_def(defs, md)
        if problems(mdefs, md):        # a compile error, not a bug the spec should catch
            ill_formed.append(desc)
            continue
        try:
            ex = not all(r["pass"] for r in run_examples(mdefs, md))
            props = bool(proptest(mdefs, md, n=n_props, seed=7)["violations"])
            # confirmed refutations, or the mutant can never succeed (the original is clean, so it can)
            prover = any(r["status"] in ("refuted", "vacuous") for r in Prover(mdefs).prove(md, first_kill=True)[0])
        except Exception as e:         # a crash in the checker is not a kill; report it
            errors.append(f"{desc}: {type(e).__name__}: {e}")
            continue
        rows.append((desc, ex, props, prover))
    return {
        "total": len(rows),
        "guarantee_killed": sum(1 for _, _, pr, pv in rows if pr or pv),
        "prover_killed": sum(1 for *_, pv in rows if pv),
        "props_killed": sum(1 for _, _, pr, _ in rows if pr),
        "example_killed": sum(1 for _, ex, _, _ in rows if ex),
        "killed": sum(1 for _, ex, pr, pv in rows if ex or pr or pv),
        "guarantee_missed": [(desc, ex) for desc, ex, pr, pv in rows if not (pr or pv)],
        "survived": [desc for desc, ex, pr, pv in rows if not (ex or pr or pv)],
        "ill_formed": ill_formed, "errors": errors,
    }
