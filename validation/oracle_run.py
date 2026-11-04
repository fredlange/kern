"""Interpreter oracle for the Rust port: what kern/run.py does on every well-formed module of the parser corpus
(validation/oracle_syntax.py), so toolchain/src/run.rs can be checked against it.

    .venv/bin/python validation/oracle_run.py OUT.json

Per module and definition, one Interp runs, in order: every `eg` (as check.run_examples does), then 40 random
calls (worlds, state and arguments drawn here and written out, so the Rust side needs no RNG of its own), then the
same calls again with check_loops on and a step budget. Per call: the outcome or the error (kind and text), the
world after, last_result, holds(), unstated_refusal(), broken_invariants() (all keys, and the written keys),
observations and loop violations. Assumed handlers use the default seeded provider, so the stream continues
across calls exactly as it does in kern/run.py.
"""
import json
import os
import random
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern.run import Fail, Interp, KernError, World  # noqa: E402
from kern.syntax import ParseError, parse_module  # noqa: E402
from kern.wf import check_module  # noqa: E402
from validation.oracle_syntax import corpus  # noqa: E402


def kind(e):
    return {KernError: "kern", KeyError: "key", TypeError: "type"}.get(type(e), type(e).__name__)


def world_json(w):
    """A snapshot (json round trip), so later appends to the world's lists don't leak into it."""
    return json.loads(json.dumps(_world_json(w)))


def _world_json(w):
    return {"bal": list(w.bal.items()), "own": list(w.own.items()), "clock": w.clock, "audit": w.audit,
            "maps": [[x, list(kv.items())] for x, kv in w.maps.items()], "out": list(w.out.items())}


def guarded(f):
    try:
        return {"ok": f()}
    except Exception as e:  # noqa: BLE001  (the kind is the point)
        return {"err": [kind(e), str(e)]}


def one_call(interp, d, args, w):
    before = w.copy()
    rec = {"args": args, "world": world_json(w)}
    n_obs = len(interp.observations)
    try:
        outcome, after = interp.run(d.name, args, w)
    except Exception as e:  # noqa: BLE001
        rec["err"] = [kind(e), str(e)]
        rec["after"] = world_json(w)
        return rec
    rec["outcome"] = [outcome[0], list(outcome[1])] if outcome != ("ok",) else ["ok"]
    rec["after"] = world_json(after)
    rec["last_result"] = getattr(interp, "last_result", None)
    rec["observations"] = interp.observations[n_obs:]
    if outcome == ("ok",):
        rec["holds"] = guarded(lambda: interp.holds(d, args, before.copy(), after.copy()))
    rec["unstated"] = guarded(lambda: interp.unstated_refusal(d, args, before, outcome))
    rec["broken"] = guarded(lambda: interp.broken_invariants(after.copy()))
    keys = {"Account": {a for a in after.bal if after.bal[a] != before.bal.get(a)},
            "Int": {k for m in after.maps.values() for k in m if isinstance(k, int)}}
    rec["keys"] = {t: sorted(v, key=str) for t, v in keys.items()}
    rec["broken_keys"] = guarded(lambda: interp.broken_invariants(after.copy(), keys))
    return rec


def random_call(defs, d, rnd):
    accts = ["A", "B", "C"]
    w = World({a: rnd.randint(0, 300) for a in accts}, {a: rnd.choice(["u1", "u2"]) for a in accts},
              rnd.randint(0, 50))
    for x, t in defs.states.items():
        keys = accts if t == "Account" else range(-3, 6)
        w.maps[x] = {k: rnd.choice([0, 0, 1, rnd.randint(-5, 300)]) for k in keys if rnd.random() < 0.8}
    args = []
    for _, t in d.params:
        r = rnd.random()
        if t == "Account":
            args.append(rnd.choice(accts) if r < 0.95 else rnd.choice(["Z", 3]))
        else:
            args.append(rnd.choice([rnd.randint(-5, 5), rnd.randint(0, 200), rnd.randint(0, 400)]) if r < 0.97
                        else "A")
    return args, w


def module_run(defs, seed):
    out = []
    for d in defs.values():
        rnd = random.Random(f"{seed}:{d.name}")
        interp, calls = Interp(defs), []
        for ex in d.examples:
            w = World.from_example(ex.world, ex.now, ex.maps)
            calls.append({"eg": ex.src, **one_call(interp, d, list(ex.call[1]), w)} if ex.call[0] == d.name else
                         {"eg": ex.src, "call": ex.call[0], **one_call(interp, defs[ex.call[0]], list(ex.call[1]), w)}
                         if ex.call[0] in defs else {"eg": ex.src, "skip": True})
        draws = [random_call(defs, d, rnd) for _ in range(40)]
        for args, w in draws:
            calls.append(one_call(interp, d, args, w.copy()))
        loop = Interp(defs)
        loop.check_loops, loop.budget = True, 2000
        for args, w in draws:
            rec = one_call(loop, d, args, w.copy())
            rec["loop_violations"] = loop.loop_violations
            loop.loop_violations = []
            calls.append({"check_loops": True, **rec})
        out.append({"def": d.name, "calls": calls})
    return out


def main(argv):
    out, n = [], 0
    for i, (name, src) in enumerate(corpus()):
        try:
            defs = check_module(parse_module(src))
        except ParseError:
            continue
        out.append({"name": name, "src": src, "defs": module_run(defs, i)})
        n += sum(len(x["calls"]) for x in out[-1]["defs"])
    with open(argv[0], "w") as f:
        json.dump(out, f, ensure_ascii=False)
    print(f"{len(out)} modules, {n} calls -> {argv[0]}")


if __name__ == "__main__":
    main(sys.argv[1:])
