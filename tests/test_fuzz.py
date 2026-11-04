"""The soundness fuzzer (kern/fuzz.py, docs/ROADMAP_RUN.md FZ): a small run is clean, and an injected
prover fault is found. The full run (seed 1, 300 + 200 programs and every template mutant) is
`python -m kern.fuzz`, reported in ROADMAP_RUN.md."""
import json
import os
import subprocess
import sys

from kern import fuzz

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _small(seed):
    results = [fuzz.check_program(j) for j in fuzz.jobs(seed, 15, 15, 60, False)]
    return fuzz.summarise(results)


def test_a_small_fuzz_run_has_no_contradictions_and_is_deterministic():
    s = _small(7)
    assert s["contradictions"] == 0 and not s["crashes"], s["programs_with_contradictions"]
    assert s["well_formed"] == s["programs"] and s["unmapped"] == 0
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
    p = subprocess.run([sys.executable, "-m", "kern.fuzz", "--loops", "0", "--handlers", "20", "--runs", "60",
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
