"""Examples, property tests and mutation testing."""
from __future__ import annotations

import copy
import dataclasses
import random

from .prove import Prover
from .run import Interp, KernError, World
from .syntax import code, stmt_code
from .wf import problems


def run_examples(defs, d):
    interp, out = Interp(defs), []
    for ex in d.examples:
        try:
            w = World.from_example(ex.world, ex.now)
            outcome, after = interp.run(ex.call[0], ex.call[1], w)
            ok = (outcome[0] == ex.outcome[0] and (ex.outcome[0] == "ok" or list(outcome[1]) == ex.outcome[1])
                  and all(after.bal[a] == b for a, b in ex.after.items()))
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
    stats = {"runs": n, "succeeded": 0, "failures": {}, "violations": []}
    for _ in range(n):
        accts = ["A", "B", "C"]
        w = World({a: rnd.randint(0, 1000) for a in accts}, {a: rnd.choice(["u1", "u2"]) for a in accts},
                  times.randint(0, 100))
        args = _rand_args(d, accts, rnd)
        before = copy.deepcopy(w)
        try:
            outcome, after = interp.run(d.name, args, w)
        except (KernError, KeyError, TypeError) as e:
            stats["violations"].append({"error": str(e), "args": args})
            continue
        if outcome == ("ok",):
            stats["succeeded"] += 1
            for kind, c in interp.holds(d, args, before, after):
                if len(stats["violations"]) < 3:
                    world = " ".join(f"{a}:{before.bal[a]}@{before.own[a]}" for a in accts) + f" now:{before.clock}"
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

SWAP = {"+": "-", "-": "+", ">=": ">", ">": ">=", "<=": "<", "<": "<=", "=": "!=", "!=": "="}


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
    body = d.body
    for i, s in enumerate(body):
        yield f"drop `{stmt_code(s)}`", body[:i] + body[i + 1:]
        exprs = {"let": [2], "need": [1], "do": [1]}[s[0]]
        for j in exprs:
            for desc, m in _expr_mutants(s[j], same_type):
                new = list(s)
                new[j] = m
                yield f"{desc} in `{stmt_code(s)}`", body[:i] + [tuple(new)] + body[i + 1:]
        if s[0] == "need":                       # the failure payload, e.g. Insufficient (amount - have)
            for k, a in enumerate(s[3]):
                for desc, m in _expr_mutants(a, same_type):
                    new = list(s)
                    new[3] = s[3][:k] + [m] + s[3][k + 1:]
                    yield f"{desc} in payload of `{stmt_code(s)}`", body[:i] + [tuple(new)] + body[i + 1:]


def mutation_score(defs, d, n_props=150):
    """Run every layer on every well-formed mutant, so the score can say *what* catches bugs.

    Guarantee strength = mutants caught by the guarantees (prover refutation confirmed in the
    interpreter, or a random test violating a guarantee). Examples are reported separately:
    they pin behaviour, but they are not the proven part of the spec."""
    rows, ill_formed, errors = [], [], []
    for desc, body in mutants(d):
        md = dataclasses.replace(d, body=body)
        mdefs = dict(defs, **{d.name: md})
        if problems(mdefs, md):        # a compile error, not a bug the spec should catch
            ill_formed.append(desc)
            continue
        try:
            ex = not all(r["pass"] for r in run_examples(mdefs, md))
            props = bool(proptest(mdefs, md, n=n_props, seed=7)["violations"])
            # confirmed refutations, or the mutant can never succeed (the original is clean, so it can)
            prover = any(r["status"] in ("refuted", "vacuous") for r in Prover(mdefs).prove(md)[0])
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
