"""Runs every check for a module and assembles one report per definition.

Trust propagates: a guarantee proven *modularly* is only as good as the callee
guarantees it relied on, so a refuted callee downgrades its callers."""
from __future__ import annotations

from .check import mutation_score, proptest, run_examples
from .infer import callees, fails, lifted, uses, writes
from .prove import Prover
from .wf import check_module


TESTED_MIN = 50          # random successes needed before an unproven guarantee shows as ◐


def _clean(r):
    return all(g["status"] == "proven" for g in r["guarantees"]) and all(e["pass"] for e in r["examples"]) \
        and not r["props"]["violations"]


def analyse(defs, mutation=True, baseline=None):
    check_module(defs)                 # nothing ill-formed reaches the prover or interpreter
    reports = {}
    for d in defs.values():
        guarantees, relied = Prover(defs).prove(d)
        reports[d.name] = {
            "def": d, "defs": defs, "hash": d.hash, "uses": uses(defs, d), "fails": fails(defs, d),
            "writes": writes(defs, d), "lifted": lifted(defs, d),
            "callees": callees(defs, d), "relied_on": relied,
            "guarantees": guarantees, "examples": run_examples(defs, d), "props": proptest(defs, d),
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
            if g["status"] == "unknown" and p["succeeded"] >= TESTED_MIN and not p["violations"]:
                g["status"], g["tested"] = "tested", p["succeeded"]
    for r in reports.values():
        if mutation and _clean(r):
            r["mutation"] = mutation_score(defs, r["def"])
        else:
            r["mutation"] = None
        r["clean"] = _clean(r)
    for r in reports.values():
        r["regression"] = []
    if baseline:
        from .baseline import compare
        compare(reports, baseline)
    return reports
