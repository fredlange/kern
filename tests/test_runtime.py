"""Rust runtime tests (docs/research/RUNTIME_PLAN.md): the compiled program against kern/run.py. Skipped
without cargo."""
import os
import shutil
import subprocess

import pytest

from kern import emit_rust
from kern.run import Interp, World
from kern.syntax import parse_module
from kern.wf import check_module
from validation import difftest

pytestmark = pytest.mark.skipif(shutil.which("cargo") is None, reason="needs cargo")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAX = 2**63 - 1


@pytest.fixture(scope="module")
def escrow():
    return emit_rust.build(emit_rust.emit(os.path.join(ROOT, "examples", "escrow", "escrow_s2.k")))


@pytest.fixture(scope="module")
def probes():
    return emit_rust.build(emit_rust.emit(os.path.join(ROOT, "tests", "probes", "runtime.k")))


def run(binary, *lines):
    r = subprocess.run([binary, "run"], input="\n".join(lines) + "\n", capture_output=True, text=True, check=True)
    return r.stdout.splitlines()


# ---- C3: Int is i64 at run time; leaving it is a fault: atomic, no clock tick, not a refusal

def test_overflow_after_a_write_rolls_back_and_does_not_tick(escrow):
    """transfer debits `from`, then the credit to `to` overflows: the debit is undone too."""
    world = f"world A:100@u1 B:{MAX - 5}@u2 now:7"
    out = run(escrow, world, "dump", "call transfer A B 10", "dump")
    assert out[1] == "fault t=7"
    assert out[0] == out[2] == f"clock=7 | A:100@u1 B:{MAX - 5}@u2 |"


def test_overflow_is_a_deliberate_divergence_from_run_py():
    """run.py has unbounded ints, so the same call succeeds there (docs/research/RUNTIME_PLAN.md says so)."""
    defs = check_module(parse_module(open(os.path.join(ROOT, "examples", "escrow", "escrow_s2.k")).read()))
    w = World({"A": 100, "B": MAX - 5}, {"A": "u1", "B": "u2"}, 7)
    assert Interp(defs).run("transfer", ["A", "B", 10], w)[0] == ("ok",)
    assert w.bal["B"] == MAX + 5


def test_overflow_in_an_expression_is_a_fault(probes):
    out = run(probes, f"world A:{MAX}@u1 B:1@u2", "call snapshot 0", "dump")
    assert out[0] == "fault t=0" and out[1] == f"clock=0 | A:{MAX}@u1 B:1@u2 |"


def test_a_refusal_still_ticks_and_a_bad_call_does_not(escrow):
    out = run(escrow, "world A:5@u1 B:0@u2 now:3", "call transfer A B 10", "call transfer A Z 1", "call transfer A B x")
    assert out == ["refused Insufficient 5 t=4", "badcall t=4", "badcall t=4"]


def test_floor_division_matches_run_py(probes):
    """(n - 7) / 2 for n < 7 rounds toward minus infinity, as in Z3 and Python."""
    out = run(probes, "world A:0@u1", "call use_half A 0", "dump")
    assert out[0] == "ok t=1"
    assert "mark.A:-4" in out[1]            # (0 - 7) / 2 = -3.5 -> -4, not -3


# ---- C1/C2 in small: every example and the probes, run.py against the binary

def test_differential_small():
    assert difftest.main(["--per-def", "100"]) == 0


def test_unsupported_is_loud_never_wrong(tmp_path):
    """A construct the generator can't compile raises Unsupported; it never emits something else."""
    p = tmp_path / "nested.k"
    p.write_text("wrap a:Account\nbody\n  audit.emit (Outer (Inner a))\n")
    with pytest.raises(emit_rust.Unsupported):
        emit_rust.emit(str(p), str(tmp_path / "out"))


@pytest.mark.parametrize("app", ["shop", "library", "lottery"])
def test_a_package_compiles_and_matches_run_py(app):
    """`kern build` takes packages: every eg line and random calls give the same results as run.py."""
    res = difftest.run_module(os.path.join(ROOT, "examples", app), 100, 1)
    assert res["egs"] > 0 and not res["eg_failures"] and res["n_divergences"] == 0, res
