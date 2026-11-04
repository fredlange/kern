"""Runs every check for a module and assembles one report per definition.

Trust propagates: a guarantee proven *modularly* is only as good as the callee
guarantees it relied on, so a refuted callee downgrades its callers."""
from __future__ import annotations

import copy
import glob
import hashlib
import os
import pickle

from .check import mutation_score, proptest, run_examples
from .infer import callees, fails, lifted, uses, writes
from .prove import Prover
from .wf import check_module


TESTED_MIN = 50          # random successes needed before an unproven guarantee shows as ◐


def _store_kept(reports, store, exclude=None):
    """The quantified module-invariant assumption about `store` is justified only if every
    definition that writes the store proves the invariant (kern-90's review of S4)."""
    for n, r in reports.items():
        if n == exclude:
            continue
        ws = r["writes"]
        writes_it = any(w.split(".")[0] == store for w in ws if "." in w) or (store == "ledger" and any("." not in w for w in ws))
        if writes_it and any(g["kind"] == "invariant" and g["status"] != "proven" for g in r["guarantees"]):
            return False
        if writes_it and any(g["kind"] == "loop" and g["clause"].startswith("module invariant") and g["status"] != "proven"
                             for g in r["guarantees"]):
            return False
    return True


def handler_sat(defs, h):
    """Can an assumed handler's spec be met? ∀ args. needs → ∃ result. ensures. Returns
    "satisfiable", "unsatisfiable" or "not checked" (Z3 gave no answer and sampling found nothing)."""
    import z3
    from .prove import Prover
    pv = Prover(defs)
    env = {p: z3.Int(p) for p, _ in h.params}
    r = z3.Int("result")
    needs = z3.And(*[z3.BoolVal(True)] + [pv.sev(n[1], env, None) for n in h.needs])
    ens = z3.And(*[z3.BoolVal(True)] + [pv.sev(c, dict(env, result=r), None, None) for c in h.ensures])
    s = z3.Solver()
    s.set("timeout", 3000)
    s.add(needs, z3.ForAll([r], z3.Not(ens)))          # some arguments with no possible result?
    got = s.check()
    if got == z3.unsat:
        return "satisfiable"
    if got == z3.sat:
        return "unsatisfiable"
    for vals in [(v,) * len(h.params) for v in (0, 1, 2, 5, 100)]:
        s = z3.Solver()
        s.add(*[env[p] == v for (p, _), v in zip(h.params, vals)], needs, z3.Not(z3.Exists([r], ens)))
        if s.check() == z3.sat:
            return "unsatisfiable"
    return "not checked"


def _clean(r):
    return all(g["status"] in ("proven", "assumed") for g in r["guarantees"]) and all(e["pass"] for e in r["examples"]) \
        and not r["props"]["violations"]


def _assumptions(reports, r, seen=()):
    """Assumed handlers a definition's proofs rest on: its own, and its callees', transitively."""
    out = {c[len("assumed "):] for c in r["relied_on"] if c.startswith("assumed ")}
    for c in r["relied_on"]:
        if c in reports and c not in seen:
            out |= _assumptions(reports, reports[c], (*seen, r["def"].name))
    return out


_TOOL = None


def tool_hash():
    """The checker's own code: a prover fix must invalidate every cached result."""
    global _TOOL
    if _TOOL is None:
        h = hashlib.sha256()
        for f in sorted(glob.glob(os.path.join(os.path.dirname(os.path.abspath(__file__)), "*.py"))):
            h.update(open(f, "rb").read())
        _TOOL = h.hexdigest()
    return _TOOL


def cache_key(defs, d, part):
    """Everything d's results can depend on. Callers' proofs read callee *specs* and what is
    inferred from callee *bodies* (lifted refusals, writes, fails, uses), so every definition d
    reaches, transitively, is in the key with its full content (bodies included)."""
    reach, todo = set(), [d.name]
    while todo:
        n = todo.pop()
        for c in callees(defs, defs[n]):
            if c not in reach:
                reach.add(c)
                todo.append(c)
    decls = repr((sorted(getattr(defs, "states", {}).items()), getattr(defs, "invariants", []),
                  getattr(defs, "channels", [])))
    parts = [tool_hash(), part, decls, d.name, d.content_hash] + [f"{n}:{defs[n].content_hash}" for n in sorted(reach)]
    return hashlib.sha256("\n".join(parts).encode()).hexdigest()


def _cache_get(cache, key):
    if not cache:
        return None
    path = os.path.join(cache, key[:2], key)
    try:
        with open(path, "rb") as f:
            got = pickle.load(f)
        return got if isinstance(got, dict) and got.get("key") == key else None
    except Exception:                  # missing or corrupt: recompute, never trust
        return None


def _cache_put(cache, key, value):
    if not cache:
        return
    path = os.path.join(cache, key[:2], key)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        pickle.dump(dict(value, key=key), f)
    os.replace(tmp, path)


def analyse(defs, mutation=True, baseline=None, cache=None):
    """Every check for every definition. cache = a directory: results are reused for definitions
    whose cache_key is unchanged (cold and warm runs give the same reports)."""
    check_module(defs)                 # nothing ill-formed reaches the prover or interpreter
    reports = {}
    for d in defs.values():
        key = cache_key(defs, d, "base")
        hit = _cache_get(cache, key)
        if hit is None:
            guarantees, relied = Prover(defs).prove(d)
            hit = {"guarantees": guarantees, "relied_on": relied, "examples": run_examples(defs, d),
                   "props": proptest(defs, d)}
            _cache_put(cache, key, hit)
            cached = False
        else:
            cached = True
        hit = copy.deepcopy(hit)       # trust propagation below edits statuses in place
        reports[d.name] = {
            "def": d, "defs": defs, "hash": d.hash, "uses": uses(defs, d), "fails": fails(defs, d),
            "writes": writes(defs, d), "lifted": lifted(defs, d),
            "callees": callees(defs, d), "relied_on": hit["relied_on"],
            "guarantees": hit["guarantees"], "examples": hit["examples"], "props": hit["props"],
            "cached": cached,
        }
    # propagate trust through modular proofs (callees are analysed independently, so one pass
    # per depth level is enough; iterate to a fixpoint for deeper call chains)
    changed = True
    while changed:
        changed = False
        for r in reports.values():
            def weak_for(g, r=r):
                own_inv = g["kind"] == "invariant" or (g["kind"] == "loop" and g["clause"].startswith("module invariant"))
                out = [c for c in r["relied_on"] if not c.startswith("assumed ") and
                       (not _store_kept(reports, c[len("invariant on "):], exclude=r["def"].name if own_inv else None)
                        if c.startswith("invariant on ") else not _clean(reports[c]))]
                # a guarantee follows from the definition's loop invariants: if one of those isn't
                # proven, neither is it (kern-90's p3)
                # an iteration step assumes *all* the loop's invariants (induction), so a "kept" goal
                # is only as good as the others; entry goals assume none of them
                if g["kind"] != "loop" or "(kept by every iteration)" in g["clause"]:
                    out += [f"loop invariant {x['clause']}" for x in r["guarantees"]
                            if x is not g and x["kind"] == "loop" and x["status"] not in ("proven", "assumed", "conditional")]
                return out
            for g in r["guarantees"]:
                weak = weak_for(g)
                if g["status"] == "proven" and weak:
                    g["status"], g["blocked_by"] = "conditional", weak
                    changed = True
                # refuted only against a broken callee spec, and the real code does not reproduce it
                elif g["status"] == "unconfirmed" and weak:
                    g["status"], g["blocked_by"], g["via_broken_spec"] = "conditional", weak, True
                    changed = True
    # ⊙ proven, assuming the declared spec of an observation (an assumed handler): not plain ✔.
    # An assumption that can't be met would make every such proof vacuous: those are `!`.
    sat = {n: handler_sat(defs, h) for n, h in getattr(defs, "handlers", {}).items()}
    for r in reports.values():
        assumed = sorted(_assumptions(reports, r))
        r["assumptions"], r["handler_sat"] = assumed, sat
        broken = [a for a in assumed if sat.get(a) == "unsatisfiable"]
        for g in r["guarantees"]:
            if g["status"] != "proven" or g["kind"] == "pre" or not assumed:
                continue                      # a call-site need is about the caller's inputs
            if broken:
                g["status"], g["blocked_by"] = "conditional", [f"assumed {a}, whose spec can't be met" for a in broken]
            else:
                g["status"], g["assuming"] = "assumed", assumed
    # ◐ tested-only: no proof, but random tests reached success often enough and found nothing
    for r in reports.values():
        p = r["props"]
        for g in r["guarantees"]:
            # not `ok otherwise`: it is about refusals, which successful runs cannot exercise
            if g["status"] == "unknown" and g["kind"] != "ok" and p["succeeded"] >= TESTED_MIN \
                    and not p["violations"]:
                g["status"], g["tested"] = "tested", p["succeeded"]
    for r in reports.values():
        r["mutation"], r["mutation_note"] = None, None
        if _clean(r):
            key = cache_key(defs, r["def"], "mutation")
            hit = _cache_get(cache, key)
            if hit is not None:        # measured for exactly this code: current, not stale
                r["mutation"] = hit["mutation"]
            elif mutation:
                r["mutation"] = mutation_score(defs, r["def"])
                _cache_put(cache, key, {"mutation": r["mutation"]})
            else:
                r["mutation_note"] = "not measured (--quick)"
        r["clean"] = _clean(r)
    for r in reports.values():
        r["regression"] = []
    if baseline:
        from .baseline import compare
        compare(reports, baseline)
    return reports
