"""Differential test: a compiled Kern module against the reference interpreter kern/run.py.

    python validation/difftest.py [--per-def N] [--seed S] [MODULE.k ...]   (default: every example + tests/probes/runtime.k)

Scorecard C1 and C2 of docs/research/RUNTIME_PLAN.md:
  C1  every `eg` line, run in the binary, gives the eg's outcome and after-world;
  C2  random worlds and calls (the same world, calls and provider values for both) give the same
      canonical line per call (outcome, refusal payload, clock, events per channel) and the same world.

Values stay small, so no run.py integer leaves i64 (overflow is a deliberate divergence, tested
separately in tests/test_runtime.py).
"""
from __future__ import annotations

import argparse
import glob
import os
import random
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from kern import emit_rust  # noqa: E402
from kern.run import Interp, KernError, World  # noqa: E402
from kern.package import load  # noqa: E402

NAMES = ["A", "B", "C", "D", "E", "H"]
INTS = [-1, 0, 1, 2, 3, 5, 7, 50, 99, 100, 101, 150, 250, 1000]
BALANCES = [0, 1, 5, 50, 99, 100, 101, 250, 1000]
MAP_VALUES = [0, 1, 2, 5, 100, 250, -1]


def show(v):
    if isinstance(v, bool):
        return str(v)
    if isinstance(v, (int, str)) or v is None:
        return str(v)
    raise emit_rust.Unsupported(f"value {v!r} (nested constructor)")


def show_event(ev):
    return " ".join(show(x) for x in ev) if isinstance(ev, tuple) else show(ev)


def world_line(bal, own, maps, now):
    items = [f"{a}:{b}@{own[a]}" for a, b in bal.items()]
    for m, kv in maps.items():
        items += [f"{m}.{k}:{v}" for k, v in kv.items()]
    return "world " + " ".join(items + [f"now:{now}"])


def dump(w, defs):
    s = f"clock={w.clock} |" + "".join(f" {a}:{b}@{w.own[a]}" for a, b in w.bal.items()) + " |"
    for m in sorted(defs.states):
        kv = {k: v for k, v in w.maps.get(m, {}).items() if v != 0}
        for k in sorted(kv, key=(str if defs.states[m] == "Account" else int)):
            s += f" {m}.{k}:{kv[k]}"
    return s


def channels(defs):
    return ["audit"] + list(getattr(defs, "channels", []))


def py_call(interp, w, defs, name, args):
    """One call through run.py: (canonical line, provider values it drew)."""
    d = defs[name]
    before = {c: len(w.events(c)) for c in channels(defs)}
    n_obs = len(interp.observations)
    try:
        outcome, _ = interp.run(name, args, w)
    except KernError as e:
        kind = "badcall" if str(e).startswith("bad call") else "fault"
        return f"{kind} t={w.clock}", [v for _, _, v in interp.observations[n_obs:]]
    if outcome == ("ok",):
        line = f"ok result={interp.last_result}" if d.ret else "ok"
    else:
        line = "refused " + " ".join([outcome[0]] + [show(a) for a in outcome[1]])
    line += f" t={w.clock}"
    for c in channels(defs):
        new = w.events(c)[before[c]:]
        if new:
            line += f" | {c}: " + ", ".join(show_event(e) for e in new)
    return line, [v for _, _, v in interp.observations[n_obs:]]


def py_args(d, toks):
    out = []
    for (_, t), tok in zip(d.params, toks):
        out.append(int(tok) if t == "Int" else str(tok))
    return out + list(toks[len(d.params):])


class Case:
    """A script for the binary plus what run.py said, line by line."""

    def __init__(self):
        self.script, self.expected, self.where = [], [], []

    def world(self, line):
        self.script.append(line)

    def call(self, ctx, toks, py_line, obs):
        if obs:
            self.script.append("obs " + " ".join(map(str, obs)))
        self.script.append("call " + " ".join(map(str, toks)))
        self.expected.append(py_line)
        self.where.append(ctx)

    def dump(self, ctx, py_dump):
        self.script.append("dump")
        self.expected.append(py_dump)
        self.where.append(ctx)


def random_world(rng, defs):
    names = rng.sample(NAMES, rng.randint(2, 5))
    bal = {a: rng.choice(BALANCES) for a in names}
    own = {a: f"u{rng.randint(1, 3)}" for a in names}
    maps = {}
    for m, key in defs.states.items():
        kv = {}
        for _ in range(rng.randint(0, 3)):
            k = rng.choice(names) if key == "Account" else rng.choice([0, 1, 2, 3, 7])
            kv[k] = rng.choice(MAP_VALUES)
        if kv:
            maps[m] = kv
    return bal, own, maps, rng.randint(0, 20)


def build_case(defs, rng, per_def, seed, provider=None):
    case = Case()
    interp = Interp(defs, provider=provider, seed=seed)
    # the module's own examples first (C1), then random worlds (C2)
    egs = []
    for d in defs.values():
        for ex in d.examples:
            bal = {a: b for a, (b, _) in ex.world.items()}
            own = {a: o for a, (_, o) in ex.world.items()}
            maps = {m: dict(kv) for m, kv in ex.maps.items()}
            case.world(world_line(bal, own, maps, ex.now))
            w = World(dict(bal), dict(own), ex.now, [], {m: dict(kv) for m, kv in maps.items()})
            name, toks = ex.call
            line, obs = py_call(interp, w, defs, name, py_args(defs[name], toks))
            ctx = f"eg {ex.src}"
            case.call(ctx, [name] + list(toks), line, obs)
            case.dump(ctx, dump(w, defs))
            egs.append((len(case.expected) - 2, ex))
    n_calls = per_def * len(defs)
    names = list(defs)
    done = 0
    while done < n_calls:
        bal, own, maps, now = random_world(rng, defs)
        wl = world_line(bal, own, maps, now)
        case.world(wl)
        w = World(dict(bal), dict(own), now, [], {m: dict(kv) for m, kv in maps.items()})
        for _ in range(15):
            name = names[done % len(names)] if done < len(names) * per_def // 2 else rng.choice(names)
            d = defs[name]
            toks = [(rng.choice(list(bal)) if rng.random() > 0.03 else "Z") if t == "Account" else rng.choice(INTS)
                    for _, t in d.params]
            line, obs = py_call(interp, w, defs, name, py_args(d, toks))
            case.call(f"{wl} | {name} {' '.join(map(str, toks))}", [name] + toks, line, obs)
            done += 1
        case.dump(wl, dump(w, defs))
    return case, egs


def eg_problems(ex, line, world_dump):
    """C1: the binary's own output against the eg line (not against run.py)."""
    want = "ok" if ex.outcome == ("ok",) else "refused " + " ".join([ex.outcome[0]] + [str(a) for a in ex.outcome[1]])
    got = line.split(" t=")[0]
    out = []
    if not (got == want or (want == "ok" and got.startswith("ok"))):
        out.append(f"outcome {got!r}, eg says {want!r}")
    items = set(world_dump.split())
    for a, b in ex.after.items():
        if not any(i.startswith(f"{a}:{b}@") for i in items):
            out.append(f"{a} should be {b}: {world_dump}")
    for m, kv in getattr(ex, "after_maps", {}).items():
        for k, v in kv.items():
            if (v != 0 and f"{m}.{k}:{v}" not in items) or (v == 0 and any(i.startswith(f"{m}.{k}:") for i in items)):
                out.append(f"{m}.{k} should be {v}: {world_dump}")
    return out


def run_module(path, per_def, seed, show_max=5):
    defs = load(path)
    crate = emit_rust.emit(path)
    binary = emit_rust.build(crate)
    rng = random.Random(seed)
    t0 = time.time()
    case, egs = build_case(defs, rng, per_def, seed)
    t_py = time.time() - t0
    with tempfile.NamedTemporaryFile("w", suffix=".script", delete=False) as f:
        f.write("\n".join(case.script) + "\n")
        script = f.name
    t0 = time.time()
    r = subprocess.run([binary, "run", script], capture_output=True, text=True)
    t_rs = time.time() - t0
    os.unlink(script)
    if r.returncode != 0:
        raise RuntimeError(f"{binary} failed: {r.stderr[-2000:]}")
    got = r.stdout.splitlines()
    div = [(i, case.where[i], case.expected[i], got[i] if i < len(got) else "<missing>")
           for i in range(max(len(got), len(case.expected)))
           if i >= len(case.expected) or i >= len(got) or got[i] != case.expected[i]]
    eg_bad = []
    for i, ex in egs:
        for p in eg_problems(ex, got[i], got[i + 1]):
            eg_bad.append(f"{ex.src}: {p}")
    calls = sum(1 for s in case.script if s.startswith("call "))
    return {"module": os.path.basename(path), "defs": len(defs), "calls": calls, "egs": len(egs),
            "eg_failures": eg_bad, "divergences": div[:show_max], "n_divergences": len(div),
            "py_s": round(t_py, 1), "rust_s": round(t_rs, 2)}


def example_modules():
    """Every example: a package directory as one module, otherwise each .k file of the project."""
    out = []
    for d in sorted(glob.glob(os.path.join(ROOT, "examples", "*", ""))):
        out += [d.rstrip(os.sep)] if os.path.exists(os.path.join(d, "kern.pkg")) else sorted(glob.glob(os.path.join(d, "*.k")))
    return out


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("modules", nargs="*")
    ap.add_argument("--per-def", type=int, default=2000)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args(argv)
    mods = a.modules or (example_modules()
                         + [os.path.join(ROOT, "tests", "probes", "runtime.k")])   # paths the examples never reach
    total = {"calls": 0, "div": 0, "eg_fail": 0, "egs": 0}
    for m in mods:
        try:
            res = run_module(m, a.per_def, a.seed)
        except emit_rust.Unsupported as e:
            print(f"{os.path.basename(m):18} UNSUPPORTED {e}")
            continue
        total["calls"] += res["calls"]
        total["div"] += res["n_divergences"]
        total["eg_fail"] += len(res["eg_failures"])
        total["egs"] += res["egs"]
        print(f"{res['module']:18} defs={res['defs']:3} calls={res['calls']:7} egs={res['egs']:3} "
              f"eg_failures={len(res['eg_failures'])} divergences={res['n_divergences']} "
              f"(run.py {res['py_s']}s, binary {res['rust_s']}s)")
        for f in res["eg_failures"][:5]:
            print("   EG  ", f)
        for i, where, want, got in res["divergences"]:
            print(f"   DIV line {i}: {where}\n        run.py: {want}\n        binary: {got}")
    print(f"TOTAL calls={total['calls']} egs={total['egs']} eg_failures={total['eg_fail']} divergences={total['div']}")
    return 0 if total["div"] == 0 and total["eg_fail"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
