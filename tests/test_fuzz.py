"""The soundness fuzzer (validation/fuzz.py, docs/research/ROADMAP_RUN.md FZ): a small run is clean, and an injected
prover fault is found. The full run (seed 1, 300 + 200 programs and every template mutant) is
`python -m validation.fuzz`, reported in ROADMAP_RUN.md."""
import json
import os
import subprocess
import sys

from validation import fuzz

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _small(seed):
    results = [fuzz.check_program(j) for j in fuzz.jobs(seed, 15, 15, 60, False)]
    return fuzz.summarise(results)


def test_a_small_fuzz_run_has_no_contradictions_and_is_deterministic():
    s = _small(7)
    assert s["contradictions"] == 0 and not s["crashes"], s["programs_with_contradictions"]
    assert s["well_formed"] >= 0.8 * s["programs"] and s["unmapped"] == 0       # FZ's bar for the generator
    assert s["tiers"]["loop"].get("proven", 0) and s["tiers"]["loop"].get("refuted", 0)   # not testing nothing
    assert _small(7) == s


def test_every_template_mutant_is_well_formed_and_checked_in_its_own_module():
    ms = fuzz.template_mutants()
    assert len(ms) > 20
    for lbl, m in ms:                             # only the mutated definition and what it calls
        name = lbl.split(": ")[0].split(":")[-1]
        assert name in m and set(m) <= {name, "transfer", "sweep_expired"}, (lbl, sorted(m))
    assert any(set(m) == {"sweep_elsewhere", "sweep_expired", "transfer"} for _, m in ms)


def test_an_injected_prover_fault_is_found(tmp_path):
    """Fault (b): the prover treats every assumed-handler value as 0. Run from a patched copy."""
    out = tmp_path / "b.json"
    env = dict(os.environ, PYTHONPATH=ROOT)
    p = subprocess.run([sys.executable, "-m", "validation.fuzz", "--loops", "0", "--handlers", "20", "--runs", "60",
                        "--no-mutants", "--jobs", "2", "--fault", "b", "--save", str(out)], cwd=ROOT, env=env,
                       capture_output=True, text=True)
    s = json.load(open(out))["summary"]
    assert s["fault"] == "b" and "kern_fault_b_" in s["package"], s["package"]
    assert s["contradictions"] > 0 and p.returncode == 1


def test_replay_budget_stops_a_huge_loop_quickly():
    """FZ finding 5: a counterexample model may pick a loop over ~10^9 keys. Replay must give up (then
    the counterexample is unconfirmed), not run for hours. Normal runs have no budget."""
    import time

    import pytest

    from kern.run import Interp, KernError, World
    from kern.syntax import parse_module
    from kern.wf import check_module
    defs = check_module(parse_module(open(os.path.join(ROOT, "tests/probes/p3.k")).read()))
    it = Interp(defs)
    it.budget, it.check_loops = 200_000, True
    t = time.time()
    with pytest.raises(KernError, match="replay too long"):
        it.run("bump", [0, 10**9], World({}, {}))
    assert time.time() - t < 5
    assert Interp(defs).run("bump", [0, 3], World({}, {}))[0] == ("ok",)


def test_the_mutant_that_hung_replay_finishes():
    import time

    from kern.report import analyse
    (mod,) = [m for lbl, m in fuzz.template_mutants()
              if lbl.startswith("examples/batch.k:advance_subs: `clock.now` → `0`")]
    t = time.time()
    analyse(mod, mutation=False)
    assert time.time() - t < 60


def test_fz2_no_kept_goal_is_proven_while_a_sibling_invariant_is_not():
    """FZ findings 6/7: tests/probes/fz2.k (loop#249). Its `aux` invariant isn't kept; every other 'kept'
    goal assumed it, so none may be ✔."""
    from kern.report import analyse
    from kern.syntax import parse_module
    r = analyse(parse_module(open(os.path.join(ROOT, "tests/probes/fz2.k")).read()), mutation=False)["f"]
    kept = [g for g in r["guarantees"] if g["kind"] == "loop" and "(kept by every iteration)" in g["clause"]]
    assert any(g["status"] != "proven" and g["clause"].startswith("aux") for g in kept)
    assert not any(g["status"] in ("proven", "assumed") for g in kept)


def test_fz_a_conditional_sibling_still_blocks_a_kept_goal(monkeypatch):
    """FZ finding 7, independent of solver luck: a sibling that is `?` (and so becomes `!`) blocks the others."""
    from kern import report
    from kern.prove import Prover
    from kern.syntax import parse_module
    src = open(os.path.join(ROOT, "tests/probes/fz2.k")).read()
    real = Prover.prove

    def fake(self, d, first_kill=False):
        gs, relied = real(self, d, first_kill)
        for g in gs:                              # make the aux 'kept' goal unconfirmed, everything else ✔
            if g["kind"] == "loop" and g["clause"].startswith("aux") and "kept" in g["clause"]:
                g["status"] = "unconfirmed"
            elif g["status"] in ("refuted", "conditional", "unknown", "unconfirmed"):
                g["status"] = "proven"
                g.pop("counterexample", None)
        return gs, relied
    monkeypatch.setattr(Prover, "prove", fake)
    r = report.analyse(parse_module(src), mutation=False)["f"]
    kept = [g for g in r["guarantees"] if g["kind"] == "loop" and "kept" in g["clause"] and not g["clause"].startswith("aux")]
    assert kept and all(g["status"] == "conditional" for g in kept), [(g["clause"][:30], g["status"]) for g in kept]
