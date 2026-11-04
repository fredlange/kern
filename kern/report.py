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


def _clean(r):
    return all(g["status"] == "proven" for g in r["guarantees"]) and all(e["pass"] for e in r["examples"]) \
        and not r["props"]["violations"]


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
            weak = [c for c in r["relied_on"] if not _clean(reports[c])]
            for g in r["guarantees"]:
                if g["status"] == "proven" and weak:
                    g["status"], g["blocked_by"] = "conditional", weak
                    changed = True
                # refuted only against a broken callee spec, and the real code does not reproduce it
                elif g["status"] == "unconfirmed" and weak:
                    g["status"], g["blocked_by"], g["via_broken_spec"] = "conditional", weak, True
                    changed = True
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
