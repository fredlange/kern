"""Soundness fuzzer for loops and assumed handlers (docs/research/ROADMAP_RUN.md, FZ).

    python -m validation.fuzz [--seed 1] [--loops 300] [--handlers 200] [--runs 300] [--no-mutants]
                              [--fault a|b|c] [--jobs 8] [--save DIR]

Generates programs (random ones, and every well-formed body mutant of the loop and handler templates),
runs the normal checker on each (`analyse(mutation=False)`), then runs every definition in the
interpreter on random small worlds. The interpreter is the oracle. A *contradiction* is a claim on the
page (✔, ⊙, or ∅ "never succeeds") that a run shows false. Faults (a)-(c) are prover bugs injected into
a temporary copy of kern/, to show the fuzzer can find that kind of bug at all.
"""
import argparse
import json
import multiprocessing
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time

from kern.infer import callees, relevant_states
from kern.report import analyse
from kern.run import Fail, Interp, KernError, World
from kern.syntax import Module, ParseError, code, parse_module, with_def
from kern.wf import check_module

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.environ.get("KERN_FUZZ_DATA") or os.path.dirname(HERE)   # templates and probes
CLAIMED = ("proven", "assumed")
TEMPLATES = ["examples/batch/batch.k", "examples/raffle/raffle.k"] + [f"tests/probes/p{i}.k" for i in range(1, 8)]

# ---------------------------------------------------------------- random loop programs

def _perturb(rnd, text, kind):
    """One small, usually wrong, change to a clause or statement."""
    options = []
    import re
    for c in ("0", "1", "2", "3", "5", "7"):            # a constant, never a map key (`hold.get' 0`)
        pat = re.compile(rf"(?<!get)(?<!get') {c}\b")
        if pat.search(text):
            options.append(lambda t, pat=pat, c=c: pat.sub(f" {max(0, int(c) + rnd.choice([-1, 1]))}", t, count=1))
    if kind == "inv":
        options += [lambda t: t.replace(" to k", " to (k + 1)"), lambda t: t.replace(" from lo", " from (lo + 1)"),
                    lambda t: t.replace(" from lo", " from 0"), lambda t: t.replace("' j = ", "' j >= ", 1)]
    if kind == "ens":
        options += [lambda t: t.replace(" to hi", " to (hi + 1)"), lambda t: t.replace(" from lo", " from (lo - 1)"),
                    lambda t: t.replace("' j = ", "' j <= ", 1)]
    if kind == "stmt":
        options += [lambda t: t.replace("set k ", "set (k + 1) ", 1), lambda t: t.replace("set k ", "set (k - 1) ", 1),
                    lambda t: t.replace(" + ", " - ", 1)]
    if " when " in text and kind != "stmt":
        options.append(lambda t: t.split(" when ")[0] + (" for" + t.split(" for", 1)[1] if " for " in t else ""))
    if not options:
        return text
    return rnd.choice(options)(text)


def gen_loop(rnd):
    """A module with one loop definition: correct invariants and ensures for its body, then perturbed.
    Modes (FZ2): "mixed" perturbs anything; "inv" has one statement and one invariant and perturbs only
    that invariant; "ens" keeps the invariants correct and perturbs only one ensure. One wrong invariant
    among several makes one goal ✗ and the rest `!`; the single-invariant modes refute directly."""
    mode = rnd.choice(["mixed", "inv", "ens"])
    L = rnd.choice(["lo", "lo", "lo", "0"])
    H = rnd.choice(["hi", "hi", "hi", "(hi - 1)"])
    c, c2 = rnd.choice([0, 1, 2, 3]), rnd.choice([1, 5, 7])
    kinds = rnd.sample(["const", "add", "cond", "acc", "copy", "need"], rnd.choice([1, 1, 2])) if mode != "inv" \
        else [rnd.choice(["const", "add", "acc", "copy"])]
    stmts, invs = [], []
    for kind in kinds:
        if kind == "const":
            stmts.append([f"hold.set k {c2}"])
            invs.append(f"hold.get' j = {c2} for j from {L} to k")
        elif kind == "add":
            stmts.append([f"hold.set k (hold.get k + {c})"])
            invs.append(f"hold.get' j = hold.get j + {c} for j from {L} to k")
        elif kind == "cond":
            stmts.append([f"if hold.get k > {c}", f"  hold.set k {c2}"])
            invs.append(f"hold.get' j = {c2} when hold.get j > {c} for j from {L} to k")
            invs.append(f"hold.get' j = hold.get j when hold.get j <= {c} for j from {L} to k")
        elif kind == "acc":
            stmts.append([f"tally.set 0 (tally.get 0 + {c2})"])
            invs.append(f"tally.get' 0 = tally.get 0 + {c2} * (k - {L})")
        elif kind == "copy":
            stmts.append(["aux.set k (hold.get k)"])
            invs.append(f"aux.get' j = hold.get j for j from {L} to k")
        elif kind == "need":
            stmts.append([f"need hold.get k >= {c} else Low"])
    if not invs or (mode != "inv" and rnd.random() < 0.3):
        invs.append("ledger.total' = ledger.total")
    ens = []
    for i in invs:
        if " for j from " in i:
            ens.append(i.rsplit(" to k", 1)[0] + f" to {H}")
        elif "* (k - " in i:
            ens.append(i.replace("(k - ", f"({H} - "))
        else:
            ens.append(i)
    needs = []
    if rnd.random() < 0.4:
        needs.append(f"need {L} <= {H} else Empty")
    other = rnd.random() < 0.25
    if other:
        if rnd.random() < 0.7:
            needs.append(f"need other < {L} or other >= {H} else InRange")
        ens.append("hold.get' other = hold.get other")
    if mode == "inv":                         # only the invariant is wrong (body and ensure correct)
        invs = [_perturb(rnd, invs[0], "inv")] + invs[1:]
    elif mode == "ens":                       # only one ensure is wrong (invariants correct)
        i = rnd.randrange(len(ens))
        ens = ens[:i] + [_perturb(rnd, ens[i], "ens")] + ens[i + 1:]
    else:
        # perturb: each item with probability 0.3 (half the programs keep their correct invariants)
        p = 0.3 if rnd.random() < 0.5 else 0.0
        invs = [_perturb(rnd, x, "inv") if rnd.random() < p else x for x in invs]
        ens = [_perturb(rnd, x, "ens") if rnd.random() < p else x for x in ens]
        stmts = [[_perturb(rnd, s[0], "stmt") if rnd.random() < p else s[0]] + s[1:] for s in stmts]
        if rnd.random() < 0.3:
            invs = rnd.sample(invs, max(1, len(invs) - 1))             # a missing invariant
    lines = ["state hold Int", "state tally Int", "state aux Int"]
    if rnd.random() < 0.6:
        lines.append("invariant o:Int hold.get o >= 0")
    lines.append("")
    lines.append("f lo:Int hi:Int" + (" other:Int" if other else ""))
    lines += [f"  {n}" for n in needs]
    if rnd.random() < 0.3:
        lines.append("  ok otherwise")
    lines += [f"  ensure {e}" for e in ens]
    lines.append("body")
    if mode == "mixed" and rnd.random() < 0.15:
        lines.append(f"  hold.set {L} {c2}")
    lines.append(f"  for k from {L} to {H}")
    lines += [f"    invariant {i}" for i in invs]
    for s in stmts:
        lines += [f"    {x}" for x in s]
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------- random assumed-handler programs

def gen_handler(rnd):
    spec = rnd.choice(["result >= 0 and result < n", "result >= 0 and result < n", "result >= 0", "result < n",
                       "result >= 1 and result <= n", "result >= 0 and result < n and result != 2",
                       "result >= n and result < n"])
    lines = ["handler rand.below n:Int -> Int"]
    if rnd.random() < 0.7:
        lines.append("  need n > 0 else BadBound")
    lines += [f"  ensure {spec}", "", "state hold Int", "state aux Int"]
    if rnd.random() < 0.4:
        lines.append("invariant o:Int hold.get o >= 0")
    c = rnd.choice([0, 0, 1, 2])
    arg = rnd.choice(["n", "n", "(n + 1)", "(n - 1)", "x", "3"])
    body = [f"r = rand.below {arg}", f"hold.set 0 (r + {c})" if c else "hold.set 0 r"]
    two = rnd.random() < 0.4
    if two:
        body += [f"s = rand.below {rnd.choice(['n', 'n', '3'])}", "hold.set 1 s"]
    if rnd.random() < 0.35:
        body += ["if r = 0", "  hold.set 2 1", "else", "  hold.set 2 2"]
    if rnd.random() < 0.3:
        body += ["for k from 0 to r", "  invariant aux.get' j = 1 for j from 0 to k", "  aux.set k 1"]
    pool = ["hold.get' 0 >= 0", f"hold.get' 0 >= {c}", "hold.get' 0 < n", f"hold.get' 0 < n + {c}",
            "hold.get' 0 <= n", f"hold.get' 0 = {c}", "hold.get' 3 = hold.get 3", "hold.get' 2 >= 1",
            "hold.get' 2 = 1"]
    if two:
        pool += ["hold.get' 1 < n", "hold.get' 1 = hold.get' 0", "hold.get' 1 >= 0"]
    if "aux.set" in "\n".join(body):
        pool += ["aux.get' 0 = 1 when hold.get 0 >= 0", "aux.get' 0 = 1"]   # `when` reads the before-state
    ens = rnd.sample(pool, rnd.choice([1, 2, 3]))
    lines += ["", "pick n:Int x:Int"]
    if rnd.random() < 0.6:
        lines.append("  need n > 0 else Bad")
    if rnd.random() < 0.3:
        lines.append("  ok otherwise")
    lines += [f"  ensure {e}" for e in ens]
    lines.append("body")
    lines += [f"  {b}" for b in body]
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------- template mutants

def _closure(defs, name):
    need, todo = set(), [name]
    while todo:
        n = todo.pop()
        if n not in need:
            need.add(n)
            todo += callees(defs, defs[n])
    return Module({n: defs[n] for n in defs if n in need}, states=defs.states, invariants=defs.invariants,
                  channels=defs.channels, handlers=defs.handlers)


def template_mutants():
    """(label, module) for every well-formed body mutant of each template definition, in a module with
    only it and its callees (so callers' trust propagation doesn't mask it)."""
    import dataclasses

    from kern.check import mutants
    from kern.wf import problems
    out = []
    for path in TEMPLATES:
        defs = check_module(parse_module(open(os.path.join(ROOT, path)).read()))
        for name, d in defs.items():
            if name == "transfer":
                continue
            for desc, body in mutants(d):
                md = dataclasses.replace(d, body=body, examples=[])
                mdefs = _closure(with_def(defs, md), name)
                if not problems(mdefs, md):
                    out.append((f"{path}:{name}: {desc}", mdefs))
    return out


# ---------------------------------------------------------------- the oracle

class _Oracle(Interp):
    """The interpreter, recording loop heads of the top-level definition only."""

    def __init__(self, defs, provider, seed):
        super().__init__(defs, provider=provider, seed=seed)
        self.depth, self.heads = 0, []

    def call_def(self, d, args, world):
        self.depth += 1
        try:
            return super().call_def(d, args, world)
        finally:
            self.depth -= 1

    def _check_loop(self, s, env, world, k):
        if self.depth != 1:
            return
        e = dict(env, **{s[1]: k})
        bad = [code(inv) for inv in s[4] if not self.ev(inv, e, self._call_pre, world)]
        bad += [f"module invariant {text}" for text, _ in self.broken_invariants(world)]
        self.heads.append((k, bad))


def _provider(defs, rnd):
    """Random values that satisfy each handler's assumed spec (None if none in range)."""
    checker = Interp(defs)

    def provide(name, args):
        h = defs.handlers[name]
        env = {p: a for (p, _), a in zip(h.params, args)}
        cands = list(range(-12, 16)) + [a + d for a in args if isinstance(a, int) for d in (-1, 0, 1)]
        rnd.shuffle(cands)
        w = World({}, {}, 0)
        for v in cands:
            if all(checker.ev(c, dict(env, result=v), w) for c in h.ensures):
                return v
        raise _NoValue(name)
    return provide


class _NoValue(Exception):
    pass


def _world(defs, d, rnd):
    accts = ["A", "B", "C", "D"]
    w = World({a: rnd.choice([0, 5, 10, 30, rnd.randint(0, 200)]) for a in accts},
              {a: rnd.choice(["u1", "u2"]) for a in accts}, rnd.randint(0, 20))
    neg = not getattr(defs, "invariants", []) and rnd.random() < 0.3
    for x, key_type in relevant_states(defs, d).items():
        keys = accts if key_type == "Account" else range(-3, 11)
        w.maps[x] = {k: rnd.choice([0, 0, 1, 2, 5, 8, rnd.randint(0, 30)] + ([-1, -4] if neg else [])) for k in keys}
    args = [rnd.choice(accts) if t == "Account" else rnd.choice(list(range(-2, 9)) + [rnd.randint(-3, 12)])
            for _, t in d.params]
    return w, args


def oracle(defs, reports, rnd, runs):
    """Contradictions between the page and the interpreter, plus run statistics."""
    found, stats = [], {"runs": 0, "ok": 0, "skipped": 0, "unmapped": 0, "unmapped_kinds": {}}
    for name, d in defs.items():
        r = reports[name]
        goals = {}
        for g in r["guarantees"]:
            goals.setdefault((g["kind"], g["clause"]), []).append(g)
        vacuous = any(g["status"] == "vacuous" for g in r["guarantees"])

        def claim(kind, clause, evidence):
            gs = goals.get((kind, clause))
            if gs is None and kind == "emit":          # the page joins events with ", then " (and names
                text = clause.replace("; ", ", then ")  # a non-audit channel first)
                gs = next((goals[k] for k in goals if k[0] == "emit" and k[1].split(" ", 1)[-1] == text
                           or k == ("emit", text)), None)
            if gs is None:                         # evidence with no goal on the page to check it against
                stats["unmapped"] += 1
                key = f"{kind}: {clause}"
                stats["unmapped_kinds"][key] = stats["unmapped_kinds"].get(key, 0) + 1
                return
            # the same text can belong to several goals (two call sites, two paths); evidence against
            # the text is a contradiction only if every one of them is claimed
            if all(g["status"] in CLAIMED for g in gs):
                found.append({"def": name, "kind": kind, "clause": clause, "status": gs[0]["status"], **evidence})
        for _ in range(runs):
            w, args = _world(defs, d, rnd)
            if Interp(defs).broken_invariants(w):
                stats["skipped"] += 1                 # the proofs assume a valid before-state
                continue
            it = _Oracle(defs, _provider(defs, rnd), rnd.randrange(10**9))
            it.check_loops = True
            before = w.copy()
            stats["runs"] += 1
            ev = {"args": args, "world": {"bal": before.bal, "maps": before.maps, "now": before.clock}}
            try:
                outcome, after = it.run(name, args, w)
            except _NoValue:
                stats["skipped"] += 1
                continue
            except KernError as e:
                msg = str(e)
                if " gave no value for " in msg:   # the provider has no value the spec allows here
                    stats["skipped"] += 1
                    continue
                if msg.startswith("need of "):
                    h, text = msg[len("need of "):].split(" not met: ")
                    claim("pre", f"{h}: need {text} (at a call)", dict(ev, saw=msg))
                else:
                    found.append({"def": name, "kind": "error", "clause": msg, "status": "-", **ev})
                outcome, after = None, None
            # a module invariant is the loop's business only if the loop writes its state (then the page has
            # a loop goal for it); an unrelated one broken before the loop is the call's `invariant` goal
            heads = [(k, [t for t in bad if not t.startswith("module invariant ")
                          or ("loop", f"{t}  (holds when the loop starts)") in goals]) for k, bad in it.heads]
            failing = next(((i, k, bad) for i, (k, bad) in enumerate(heads) if bad), None)
            if failing:
                i, k, bad = failing
                part = "holds when the loop starts" if i == 0 else "kept by every iteration"
                for text in bad:
                    claim("loop", f"{text}  ({part})", dict(ev, saw=f"false at k = {k}", observed=it.observations))
            if outcome is None:
                continue
            if outcome == ("ok",):
                stats["ok"] += 1
                if vacuous:
                    found.append({"def": name, "kind": "vacuous", "clause": "never succeeds", "status": "vacuous", **ev})
                for kind, clause in it.holds(d, args, before, after):
                    claim(kind, clause, dict(ev, saw="false after an ok run", observed=it.observations))
                for text, key in it.broken_invariants(after):
                    claim("invariant", text, dict(ev, saw=f"broken at {key}"))
            elif it.unstated_refusal(d, args, before, outcome):
                claim("ok", _ok_clause(r), dict(ev, saw=f"refused {outcome[0]}"))
    return found, stats


def _ok_clause(r):
    return next((g["clause"] for g in r["guarantees"] if g["kind"] == "ok"), "ok otherwise")


# ---------------------------------------------------------------- one program

def check_program(job):
    """job = (label, source or None, module or None, seed, runs) -> result dict. With KERN_FUZZ_TRACE=file,
    each program's label is logged, and a program still running after 90 s dumps its stack there."""
    trace = os.environ.get("KERN_FUZZ_TRACE")
    if not trace:
        return _check_program(job)
    import faulthandler
    with open(trace, "a") as f:
        f.write(f"start {os.getpid()} {job[0]}\n")
        f.flush()
        faulthandler.dump_traceback_later(90, file=f)
        try:
            return _check_program(job)
        finally:
            faulthandler.cancel_dump_traceback_later()
            f.write(f"end {os.getpid()} {job[0]}\n")


def _check_program(job):
    label, src, mod, seed, runs = job
    rnd = random.Random(seed)
    try:
        defs = mod if mod is not None else check_module(parse_module(src))
    except (ParseError, Exception) as e:
        return {"label": label, "wf": False, "error": f"{type(e).__name__}: {e}"[:200]}
    try:
        reports = analyse(defs, mutation=False)
    except Exception as e:                        # a checker crash is a finding too
        return {"label": label, "wf": True, "crash": f"{type(e).__name__}: {e}"[:300], "src": src}
    tiers = {}
    for r in reports.values():
        for g in r["guarantees"]:
            tiers.setdefault(g["kind"], {}).setdefault(g["status"], 0)
            tiers[g["kind"]][g["status"]] += 1
    found, stats = oracle(defs, reports, rnd, runs)
    if mod is not None:
        src = "\n".join(_source(defs))
    return {"label": label, "wf": True, "tiers": tiers, "stats": stats, "found": found[:5],
            "n_found": len(found), "src": src if found else None}


def _source(defs):
    from kern.syntax import stmt_code
    out = [f"# {n}: {', '.join(stmt_code(s) for s in d.body)[:400]}" for n, d in defs.items()]
    return out


# ---------------------------------------------------------------- the run

def jobs(seed, n_loops, n_handlers, runs, with_mutants):
    rnd = random.Random(seed)
    out = [(f"loop#{i}", gen_loop(rnd), None, rnd.randrange(10**9), runs) for i in range(n_loops)]
    out += [(f"handler#{i}", gen_handler(rnd), None, rnd.randrange(10**9), runs) for i in range(n_handlers)]
    if with_mutants:
        out += [(label, None, mod, rnd.randrange(10**9), runs) for label, mod in template_mutants()]
    return out


def summarise(results):
    s = {"programs": len(results), "well_formed": sum(r["wf"] for r in results),
         "crashes": [r["label"] for r in results if r.get("crash")],
         "contradictions": sum(r.get("n_found", 0) for r in results),
         "programs_with_contradictions": [r["label"] for r in results if r.get("n_found")],
         "tiers": {}, "runs": 0, "ok_runs": 0, "skipped": 0, "unmapped": 0, "unmapped_kinds": {}}
    for r in results:
        for kind, st in r.get("tiers", {}).items():
            for status, n in st.items():
                s["tiers"].setdefault(kind, {}).setdefault(status, 0)
                s["tiers"][kind][status] += n
        for key, n in r.get("stats", {}).get("unmapped_kinds", {}).items():
            s["unmapped_kinds"][key] = s["unmapped_kinds"].get(key, 0) + n
        for k_in, k_out in (("runs", "runs"), ("ok", "ok_runs"), ("skipped", "skipped"), ("unmapped", "unmapped")):
            s[k_out] += r.get("stats", {}).get(k_in, 0)
    for fam in ("loop#", "handler#"):
        rs = [r for r in results if r["label"].startswith(fam)]
        s[f"{fam[:-1]}_well_formed"] = f"{sum(r['wf'] for r in rs)}/{len(rs)}"
    return s


FAULTS = {   # file, exact text, replacement: prover bugs for the "teeth" condition (FZ 2)
    "a": ("report.py", 'if g["kind"] != "loop" or "(kept by every iteration)" in g["clause"]:', "if False:"),
    "b": ("prove.py", 'r = fresh_int(name.replace(".", "_"))', "r = z3.IntVal(0)"),
    "c": ("prove.py", "K = z3.If(hi >= lo, hi, lo)", "K = hi"),
}


def run_with_fault(fault, argv):
    """Copy kern/ (and this fuzzer), inject one fault, and run the fuzzer from the copy in a subprocess."""
    tmp = tempfile.mkdtemp(prefix=f"kern_fault_{fault}_")
    for d in ("kern", "validation"):
        shutil.copytree(os.path.join(os.path.dirname(HERE), d), os.path.join(tmp, d),
                        ignore=shutil.ignore_patterns("__pycache__"))
    f, old, new = FAULTS[fault]
    p = os.path.join(tmp, "kern", f)
    s = open(p).read()
    assert s.count(old) == 1, f"fault {fault}: the text to replace is not unique in {f}"
    open(p, "w").write(s.replace(old, new))
    rest = [a for i, a in enumerate(argv) if a != "--fault" and (i == 0 or argv[i - 1] != "--fault")]
    env = dict(os.environ, PYTHONPATH=tmp, KERN_FUZZ_DATA=ROOT)
    # run from the copy: `python -m` puts the working directory first on sys.path
    try:
        return subprocess.call([sys.executable, "-m", "validation.fuzz", *rest, "--injected", fault], cwd=tmp, env=env)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    ap = argparse.ArgumentParser(prog="validation.fuzz")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--loops", type=int, default=300)
    ap.add_argument("--handlers", type=int, default=200)
    ap.add_argument("--runs", type=int, default=300)
    ap.add_argument("--no-mutants", action="store_true")
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    ap.add_argument("--fault", choices=sorted(FAULTS))
    ap.add_argument("--injected", help=argparse.SUPPRESS)
    ap.add_argument("--save", help="write the summary and every contradiction (with its program) here, as JSON")
    a = ap.parse_args(argv)
    if a.fault:
        return run_with_fault(a.fault, argv)
    js = jobs(a.seed, a.loops, a.handlers, a.runs, not a.no_mutants)
    t0 = time.time()
    # a fresh process per program: Z3's choices depend on earlier solver calls in the same process
    with multiprocessing.Pool(a.jobs, maxtasksperchild=1) as pool:
        results = pool.map(check_program, js, chunksize=1)
    s = summarise(results)
    s.update(seed=a.seed, fault=a.injected, package=os.path.dirname(sys.modules["kern"].__file__), seconds=round(time.time() - t0, 1))
    print(json.dumps(s, indent=1, sort_keys=True))
    if a.save:
        json.dump({"summary": s, "found": [r for r in results if r.get("n_found") or r.get("crash")]},
                  open(a.save, "w"), indent=1, default=str)
    return 1 if s["contradictions"] or s["crashes"] else 0


if __name__ == "__main__":
    sys.exit(main())
