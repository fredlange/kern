"""Adversarial tests of the Kern prototype against docs/FUNDAMENTALS.md.

One section per fundamental. Each test tries to BREAK the fundamental. A test that
exposes a real gap stays in, marked xfail(strict=True): it documents the gap, and
it starts failing loudly (XPASS) the day the gap is closed. Findings: docs/AUDIT.md.

Run: .venv/bin/pytest            (from the repo root; pytest.ini sets pythonpath)
"""
from __future__ import annotations

import copy
import dataclasses
import random
import re

import pytest
import z3

from kern import baseline, check, render, views
from kern import run as runmod
from kern import why as whymod
from kern.infer import callees, fails, lifted, uses, writes
from kern.prove import Prover
from kern.report import analyse
from kern.run import Interp, KernError, World
from kern.syntax import ParseError, code, parse_example, parse_expr, parse_module
from kern.wf import WellFormedError, check_module

V3 = open("examples/ledger_v3.k").read()
V4 = open("examples/ledger_v4.k").read()
LARGE = open("examples/ledger_large.k").read()
NO_WHY = {"count": 0, "stale": False, "entries": []}


def gap(reason):
    return pytest.mark.xfail(reason=reason, strict=True)


def analysed(src, mutation=False):
    defs = parse_module(src)
    return defs, analyse(defs, mutation=mutation)


def statuses(rep):
    return [g["status"] for g in rep["guarantees"]]


def no_whys(defs):
    return {n: NO_WHY for n in defs}


@pytest.fixture(scope="module")
def large():
    return analysed(LARGE)


# =====================================================================================
# F1. Optimise for the reader: views load the minimum, never a callee body
# =====================================================================================

def test_f1_editor_view_never_contains_a_callee_body(large):
    defs, reps = large
    for name, d in defs.items():
        text = views.editor(reps, name, no_whys(defs))
        own = set(d.src_lines["body"])
        for c in callees(defs, d):
            for line in defs[c].src_lines["body"]:
                assert line in own or line not in text, f"{name}'s editor view leaks {c}'s body: {line}"


def test_f1_spec_view_says_when_each_failure_happens():                # fixed (step 8)
    """Was: Insufficient appeared in `fails` but its condition lived only in the body."""
    defs, reps = analysed(V4)
    text = views.spec(reps["transfer"], NO_WHY)
    assert "body need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)" in text
    for n in defs["transfer"].needs:
        assert code(n[1]) in text


def test_f1_lifting_stops_at_the_first_write():
    """payout's second transfer needs `balance from >= fee` *after* the first transfer debited
    from, so it cannot be stated in payout's pre-state and must not be lifted. Owner checks
    read nothing that is ever written, so they are lifted from both calls."""
    defs = parse_module(V4)
    got = {code(c) for c, *_ in __import__("kern.infer", fromlist=["lifted"]).lifted(defs, defs["payout"])}
    assert "ledger.balance from >= amount" in got and "ledger.balance from >= fee" not in got
    assert {"ledger.owner from = ledger.owner house", "fee > 0", "from != house"} <= got


def test_f1_modular_proof_assumes_lifted_conditions():
    """settle's spec says nothing about from != to, but the mutant `payout to to ...` can never
    succeed (transfer refuses SameAccount). With lifted conditions the prover sees that."""
    defs = parse_module(LARGE)
    d = defs["settle"]
    body = [("do", parse_expr("payout to to house amount fee"))] + d.body[1:]
    md = dataclasses.replace(d, body=body)
    assert {g["status"] for g in Prover(dict(defs, settle=md)).prove(md)[0]} == {"vacuous"}


# =====================================================================================
# F2. Verification licenses not reading: modular proofs, callee specs only
# =====================================================================================

def test_f2_caller_proof_does_not_read_callee_body():
    defs = parse_module(V4)
    before = statuses({"guarantees": Prover(defs).prove(defs["payout"])[0]})
    t = defs["transfer"]
    # same inferred interface (uses), completely different behaviour. The prover does read the
    # callee's *inferred* `uses`, which is derived from the body: interface layer, not body.
    garbage = dataclasses.replace(t, body=[("do", parse_expr("ledger.credit from 999")),
                                           ("do", parse_expr("ledger.debit to 1")),
                                           ("do", parse_expr("audit.emit (Junk clock.now)"))])
    garbage = dataclasses.replace(garbage, body=[("let", "have", parse_expr("ledger.balance from")),
                                                 ("need", parse_expr("have >= amount"), "Insufficient",
                                                  [parse_expr("amount - have")])] + garbage.body)
    # the full inferred interface a modular call reads: uses, writes and lifted refusal conditions
    assert uses(defs, garbage) == uses(defs, t) and writes(defs, garbage) == writes(defs, t)
    assert lifted(defs, garbage) == lifted(defs, t)
    after = statuses({"guarantees": Prover(dict(defs, transfer=garbage)).prove(defs["payout"])[0]})
    assert before == after == ["proven"] * 3


def test_f2_hidden_callee_failure_rolls_back_whole_call():
    """payout's 2nd transfer fails on a body-level need its spec does not mention."""
    defs = parse_module(V4)
    w = World({"A": 100, "B": 0, "H": 0}, {"A": "u1", "B": "u1", "H": "u1"})
    snap = copy.deepcopy(w)
    out, after = Interp(defs).run("payout", ["A", "B", "H", 100, 5], w)
    assert out == ("Insufficient", [5])
    # the caller's own object is rolled back, not just the returned value (fixed, step 2)
    assert w.bal == snap.bal and w.audit == snap.audit and w.clock == snap.clock
    assert after.bal == snap.bal


def test_f2_modular_proof_agrees_with_execution_on_hidden_failures():
    defs = parse_module(V4)
    stats = check.proptest(defs, defs["payout"], n=2000, seed=11)
    assert stats["failures"].get("Insufficient", 0) > 0 and stats["violations"] == []


SHADOW = """
sneaky to:Account amount:Int
  need amount > 2000 else Small
  ensure ledger.balance' to = ledger.balance to + amount
body
  amount = amount + 1
  ledger.credit to amount
"""


def test_f2_shadowed_parameter_cannot_fake_a_proof():                   # fixed (step 3)
    """Was: ✔ and a green badge while the body credits amount+1. Now rejected before proving."""
    with pytest.raises(WellFormedError, match="`amount` shadows an existing name"):
        analysed(SHADOW)


def test_f2_spec_cannot_read_body_lets():
    with pytest.raises(WellFormedError, match="unknown name `have`"):
        analysed(V4.replace("  keep ledger.total\n  ensure ledger.balance' from", "  keep have\n  ensure ledger.balance' from"))


def test_f2_shadowed_parameter_really_violates_the_spec():
    """Companion: the interpreter confirms the ✔ above is wrong."""
    defs = parse_module(SHADOW)
    pre = World({"A": 0}, {"A": "u1"})
    out, post = Interp(defs).run("sneaky", ["A", 3000], copy.deepcopy(pre))
    assert out == ("ok",) and post.bal["A"] == 3001
    assert Interp(defs).holds(defs["sneaky"], ["A", 3000], pre, post) != []


RECURSIVE = """
loop a:Account
  ensure ledger.balance' a = ledger.balance a + 1000
body
  loop a
"""


def test_f2_self_recursion_does_not_prove_by_assumption():              # fixed (step 3)
    """Was: proven by assuming its own spec. Now rejected until there are termination measures."""
    with pytest.raises(WellFormedError, match=r"recursive \(loop -> loop\)"):
        analysed(RECURSIVE)


def test_f2_mutual_recursion_is_reported_not_crashed():                 # fixed (step 3)
    with pytest.raises(WellFormedError, match=r"recursive \(ping -> pong -> ping\)"):
        analysed("""
ping a:Account
  ensure ledger.balance' a = ledger.balance a + 7
body
  pong a
pong a:Account
  ensure ledger.balance' a = ledger.balance a + 7
body
  ping a
""")


def test_f2_runtime_error_mid_body_is_atomic():                         # fixed (step 2)
    defs = parse_module("t from:Account to:Account amount:Int\nbody\n  ledger.debit from amount\n"
                        "  ledger.credit to amount\n")
    w = World({"A": 100}, {"A": "u1"})
    with pytest.raises(KernError):
        Interp(defs).run("t", ["A", "Z", 10], w)
    assert w.bal == {"A": 100}


# =====================================================================================
# F3. The spec is the primary artifact: review guarantees, not diffs
# =====================================================================================

STEAL = """
transfer from:Account to:Account x:Account y:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need x != from else Alias
  need x != to else Alias
  need y != from else Alias
  need y != to else Alias
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  eg A:500@u1 B:0@u1 C:10@u1 D:0@u1 | transfer A B C D 100 -> ok | A:400 B:100
body
  ledger.debit from amount
  ledger.credit to amount
  ledger.debit x 1
  ledger.credit y 1
"""


def test_f3_guarantees_reveal_extra_writes():                           # fixed (step 7)
    """Was: 'All guarantees proven' and nothing on the page said x and y change. The guarantees
    are still all proven (they are true), but the inferred write set and the gap are now shown."""
    defs, reps = analysed(STEAL)
    r = reps["transfer"]
    assert r["writes"] == {"from", "to", "x", "y"}
    assert views.unguaranteed_writes(r) == ["x", "y"]
    assert "writes from to x y" in views.iface(r, NO_WHY)
    assert "No guarantee says what the new balance of <code>x</code>, <code>y</code> is." in render.card(r, NO_WHY)


def test_f3_inferred_frame_proves_a_two_callee_definition(large):
    """rebalance was shown ✗ (then ?) because withdraw's spec said nothing about `to`. The inferred
    write set says withdraw only changes `from`, so the modular proof now goes through."""
    _, reps = large
    assert statuses(reps["rebalance"]) == ["proven", "proven"] and reps["rebalance"]["clean"]


def test_f3_write_inference_through_callees_and_lets():
    defs = parse_module(LARGE + "\nalias a:Account b:Account n:Int\nbody\n  x = a\n  ledger.credit x n\n  transfer b a n\n")
    assert writes(defs, defs["alias"]) == {"a", "b"}
    assert writes(defs, defs["month_end"]) == {"from", "to", "house"}


WEAK_V4 = V4.replace("  ensure ledger.balance' to = ledger.balance to + amount\n", "") \
            .replace("  ensure ledger.balance' from = ledger.balance from - amount\n", "") \
            .replace("  keep ledger.total\n  emit TransferDone from to", "  emit TransferDone from to")


def test_f3_removed_guarantees_are_surfaced(tmp_path):                   # fixed (step 6)
    base = tmp_path / "ledger.strength"
    baseline.write(base, analysed(V4, mutation=True)[1])
    defs = parse_module(WEAK_V4)
    reps = analyse(defs, baseline=baseline.load(base))
    assert render._status(reps["transfer"]) == ("warn", "Weaker than baseline")
    msg = " ".join(reps["transfer"]["regression"])
    assert "keep ledger.total" in msg and "guarantee strength fell from 9/13 to 3/13" in msg
    # payout's text did not change, but its proof rested on transfer's deleted ensures
    assert reps["payout"]["regression"] == ["was clean at the baseline, is not now"]


# =====================================================================================
# F4. Derive, don't declare: uses / fails inference
# =====================================================================================

def _observe(defs, n_runs=300, seed=3):
    observed, failed, cur = {n: set() for n in defs}, {n: set() for n in defs}, [None]
    orig = dict(runmod.HANDLERS)
    try:
        for op, (ar, lab, fn) in orig.items():
            def spy(w, *a, lab=lab, fn=fn):
                observed[cur[0]].add(lab)
                return fn(w, *a)
            runmod.HANDLERS[op] = (ar, lab, spy)
        rnd = random.Random(seed)
        for n, d in defs.items():
            cur[0] = n
            for _ in range(n_runs):
                w = World({a: rnd.randint(0, 300) for a in "ABC"}, {a: rnd.choice(["u1", "u2"]) for a in "ABC"})
                args = [rnd.choice("ABC") if t == "Account" else rnd.randint(-3, 200) for _, t in d.params]
                out, _ = Interp(defs).run(n, args, w)
                if out != ("ok",):
                    failed[n].add(out[0])
    finally:
        runmod.HANDLERS.clear()
        runmod.HANDLERS.update(orig)
    return observed, failed


def test_f4_observed_effects_and_failures_are_inferred():
    defs = parse_module(LARGE)
    observed, failed = _observe(defs)
    for n, d in defs.items():
        obs = observed[n] - ({"ledger.read"} if "ledger.write" in observed[n] else set())
        assert obs <= uses(defs, d), n
        assert failed[n] <= set(fails(defs, d)), n


def test_f4_inference_is_transitive_through_four_levels():
    defs = parse_module(LARGE)
    d = defs["month_end"]          # month_end -> settle -> payout -> transfer
    assert uses(defs, d) == {"ledger.write", "clock", "audit"}
    assert set(fails(defs, d)) == {"FeeTooLarge", "BadAmount", "SameAccount", "SameOwner", "Insufficient"}


def test_f4_handler_inside_constructor_inside_failure_payload_is_inferred():
    defs = parse_module("f a:Account n:Int\nbody\n  need n > 0 else Late (clock.now)\n  ledger.credit a n\n")
    assert uses(defs, defs["f"]) == {"ledger.write", "clock"}


def test_f4_bad_calls_are_rejected_before_anything_runs():               # fixed (step 9)
    """Was: KernError mid-body (after the debit), an outcome outside `fails`. Now an unknown
    account is a bad *call*, rejected at the boundary; inside a body it cannot arise (types)."""
    defs = parse_module(V4)
    w = World({"A": 500}, {"A": "u1"})
    snap = copy.deepcopy(w)
    for args in (["A", "Z", 10], ["A", "A", "10"], ["A", 5, 10], ["A", "A"]):
        with pytest.raises(KernError, match="bad call"):
            Interp(defs).run("transfer", args, w)
        assert w.bal == snap.bal and w.audit == snap.audit and w.clock == snap.clock


def test_f4_every_outcome_of_a_valid_call_is_ok_or_an_inferred_failure():
    defs = parse_module(LARGE)
    rnd = random.Random(5)
    for n, d in defs.items():
        for _ in range(200):
            w = World({a: rnd.randint(0, 300) for a in "ABC"}, {a: rnd.choice(["u1", "u2"]) for a in "ABC"})
            args = [rnd.choice("ABC") if t == "Account" else rnd.randint(-3, 200) for _, t in d.params]
            out, _ = Interp(defs).run(n, args, w)
            assert out == ("ok",) or out[0] in fails(defs, d)


def test_f4_effectful_spec_is_rejected_cleanly():                       # fixed (step 3)
    with pytest.raises(WellFormedError, match="specs may only read state"):
        analysed("g a:Account\n  ensure ledger.debit a 5\nbody\n  ledger.credit a 0\n")


# =====================================================================================
# F5. Evidence over assertion: the page claims no more than the checks established
# =====================================================================================

def test_f5_no_guarantees_is_not_labelled_all_proven():          # fixed in this audit
    _, reps = analysed("nospec a:Account amount:Int\nbody\n  ledger.debit a amount\n")
    assert render._status(reps["nospec"]) == ("warn", "No guarantees stated")


class NoAnswer(z3.Solver):
    def check(self, *a):
        return z3.unknown


def test_f5_prover_timeout_with_enough_random_successes_is_tested_only(monkeypatch):   # step 5
    monkeypatch.setattr(z3, "Solver", NoAnswer)
    _, reps = analysed(V4)
    assert set(statuses(reps["transfer"])) == {"tested"}            # ◐: 96 random successes, 0 violations
    assert render._status(reps["transfer"]) == ("warn", "Tested only, not proven")
    assert not reps["transfer"]["clean"] and reps["transfer"]["mutation"] is None


def test_f5_prover_timeout_without_test_support_is_unknown(monkeypatch):           # fixed in the audit
    monkeypatch.setattr(z3, "Solver", NoAnswer)
    _, reps = analysed("rare to:Account amount:Int\n  need amount > 2000 else Small\n"
                       "  ensure ledger.balance' to = ledger.balance to + amount\nbody\n  ledger.credit to amount\n")
    assert statuses(reps["rare"]) == ["unknown"]                      # 0 random successes: no ◐
    assert render._status(reps["rare"]) == ("warn", "Not proven: prover gave no answer")


def test_f5_vacuous_spec_is_flagged():                                  # fixed (step 5)
    _, reps = analysed("never a:Account amount:Int\n  need amount > 0 else A\n  need amount < 0 else B\n"
                       "  ensure ledger.balance' a = ledger.balance a + 999\nbody\n  ledger.debit a amount\n")
    assert render._status(reps["never"]) == ("fail", "Never succeeds")
    assert statuses(reps["never"]) == ["vacuous"] and "its own needs" in reps["never"]["guarantees"][0]["reason"]


def test_f5_vacuity_from_a_callee_spec_is_attributed_to_the_callee():
    _, reps = analysed("""
liar a:Account
  ensure ledger.balance' a = ledger.balance a + 1
  ensure ledger.balance' a = ledger.balance a + 2
body
  ledger.credit a 1
user a:Account
  ensure ledger.balance' a = ledger.balance a + 100
body
  liar a
""")
    g = reps["user"]["guarantees"][0]
    assert g["status"] == "vacuous" and "specs of liar" in g["reason"]
    assert statuses(reps["liar"]) == ["proven", "refuted"]           # the callee is where the bug is


WEAK_CALLEE = """
take from:Account amount:Int
  need amount > 0 else BadAmount
  ensure ledger.balance' from = ledger.balance from - amount
body
  ledger.debit from amount
give to:Account amount:Int
  need amount > 0 else BadAmount
  ensure ledger.balance' to = ledger.balance to + amount
body
  ledger.credit to amount
move from:Account to:Account amount:Int
  need from != to else SameAccount
  keep ledger.total
body
  take from amount
  give to amount
"""


def test_f5_unconfirmed_refutation_is_not_shown_as_broken():           # fixed (step 5)
    """`take`'s spec says nothing about ledger.total, so `move`'s keep is refuted from specs alone,
    but `move` really does keep the total: the counterexample doesn't replay."""
    _, reps = analysed(WEAK_CALLEE)
    g = reps["move"]["guarantees"][0]
    assert not g["counterexample"]["confirmed_by_interpreter"]   # the evidence
    assert g["status"] == "unconfirmed"
    assert render._status(reps["move"]) == ("warn", "Not proven: counterexample not reproduced")
    assert reps["move"]["props"]["violations"] == []


def test_f5_unconfirmed_refutation_says_so():                           # fixed in the audit
    defs, reps = analysed(WEAK_CALLEE)
    assert "not reproduced in the interpreter" in render.terminal(reps, no_whys(defs))
    assert "Not reproduced in the interpreter, so this is not evidence of a bug" in render.card(reps["move"], NO_WHY)


def test_f5_inherited_failures_are_attributed_to_the_right_callee(large):   # fixed in the audit
    """Since step 8 inherited failures are shown with their condition and origin."""
    defs, reps = large
    rows = re.findall(r"<tr>(.*?)</tr>", render.card(reps["rebalance"], NO_WHY))
    text = [re.sub(r"<[^>]+>", "", row) for row in rows]
    assert "balance of from &lt; amount→ Insufficient (from withdraw)" in text   # `<` was unescaped HTML
    assert not any("Insufficient" in t and "deposit" in t for t in text)


@pytest.mark.parametrize("src", ["ledger.balance a - (amount - fee)", "a - (b + c)", "(a = b) = (c = d)",
                                 "x + (y - 1) - (z - (w + 2))", "ledger.balance' a = ledger.balance a + (b - b)"])
def test_f5_printed_clauses_mean_what_was_written(src):              # fixed in this audit
    e = parse_expr(src)
    assert parse_expr(code(e)) == e


def test_f5_tested_only_tier_exists():                                  # fixed (step 5)
    assert render.ICON["tested"] == "◐"


@gap("Tier ○ runtime-checked-only is not implemented (needs runtime checks, roadmap 4).")
def test_f5_runtime_only_tier_exists():
    assert "○" in render.ICON.values()


def test_f5_spec_strength_drops_when_guarantees_are_deleted():          # fixed (step 6)
    """Was: 13/13 either way, because examples killed everything first."""
    full = analysed(V4, mutation=True)[1]["transfer"]["mutation"]
    gone = analysed(WEAK_V4, mutation=True)[1]["transfer"]["mutation"]
    assert (full["guarantee_killed"], gone["guarantee_killed"]) == (9, 3)   # 3 = the audit mutants `emit` still catches
    assert full["killed"] == gone["killed"] == 13        # the any-layer number cannot tell them apart


# =====================================================================================
# F6. Separate knowledge by role: why is lazy, hash-anchored, never obligatory
# =====================================================================================

def test_f6_body_change_marks_why_stale():
    store = whymod.load("examples/ledger_v4.why")
    fresh = parse_module(V4)
    assert not whymod.status(store, fresh["transfer"], fresh)["stale"]
    defs = parse_module(V4.replace("  ledger.credit to amount\n  audit", "  ledger.credit to amount\n"
                                   "  ledger.credit to 0\n  audit"))
    assert whymod.status(store, defs["transfer"], defs)["stale"]


def test_f6_eg_only_change_does_not_mark_why_stale():
    """Decision: examples are evidence, not semantics, so why stays fresh."""
    store = whymod.load("examples/ledger_v4.why")
    defs = parse_module(V4.replace("transfer A B 100 -> ok | A:400 B:100", "transfer A B 7 -> ok | A:493 B:7"))
    assert not whymod.status(store, defs["transfer"], defs)["stale"]


def test_f6_hash_distinguishes_definitions_with_different_examples():   # fixed (step 10)
    """`hash` (semantics, why anchoring) ignores examples on purpose; `content_hash` (the store
    key for roadmap 2) does not."""
    a = parse_module(V4)["transfer"]
    b = parse_module(V4.replace("transfer A B 100 -> ok | A:400 B:100", "transfer A B 7 -> ok | A:493 B:7"))["transfer"]
    assert a.hash == b.hash and a.content_hash != b.content_hash


def test_f6_callee_interface_change_marks_caller_why_stale():           # fixed (step 10)
    """payout's text is identical in v1 and v2 (same `hash`), but its inferred interface gains
    SameAccount and `from != to`; why entries anchor to the interface too."""
    v1, v2 = parse_module(open("examples/ledger_v1.k").read()), parse_module(open("examples/ledger_v2.k").read())
    assert v1["payout"].hash == v2["payout"].hash
    assert whymod.anchor_hash(v1, v1["payout"]) != whymod.anchor_hash(v2, v2["payout"])
    store = whymod.load("examples/ledger.why")                        # anchored to v3 (= v2 for payout)
    assert whymod.status(store, v1["payout"], v1)["stale"] and not whymod.status(store, v2["payout"], v2)["stale"]


def test_f6_orphaned_why_entries_are_reported(tmp_path):                # fixed (step 10)
    store = whymod.load("examples/ledger_v4.why")
    defs = parse_module(V4.replace("transfer", "move"))
    assert list(whymod.orphans(store, defs)) == ["transfer"]
    path = tmp_path / "x.why"
    path.write_text(open("examples/ledger_v4.why").read())
    left = whymod.anchor(str(path), defs)
    assert list(left) == ["transfer"] and "transfer@" in path.read_text()      # kept, and reported
    reps = analyse(defs, mutation=False)
    whys = {n: whymod.status(store, d, defs) for n, d in defs.items()}
    assert "ORPHANED why: 2 entries for `transfer`" in render.terminal(reps, whys, whymod.orphans(store, defs))


def test_f6_obligation_phrased_why_is_flagged():                        # fixed (step 10)
    store = whymod.load("examples/ledger_v4.why")
    defs = parse_module(V4)
    st = whymod.status(store, defs["transfer"], defs)
    assert st["obligations"] == [e for e in st["entries"] if "must" in e] != []
    assert whymod.status(store, defs["payout"], defs)["obligations"] == []


# =====================================================================================
# F7. Closed world of effects: everything goes through handlers
# =====================================================================================

@pytest.mark.parametrize("body", ["__import__ 1", "os.system 1", "from.__class__", "print 1"])
def test_f7_body_cannot_reach_python(body):
    defs = parse_module(f"f from:Account to:Account\nbody\n  {body}\n")
    with pytest.raises(KernError):
        Interp(defs).run("f", ["A", "B"], World({"A": 1, "B": 2}, {"A": "u", "B": "u"}))


def test_f7_unknown_names_are_rejected_before_proving():                # fixed (step 3)
    with pytest.raises(WellFormedError, match="unknown name `os.system`"):
        analysed("f from:Account to:Account\nbody\n  os.system 1\n")


def test_f7_values_do_not_leak_python_semantics():                      # fixed (step 4)
    """Was: `a + b` on Accounts = 'AB' at runtime, integer addition in the prover."""
    with pytest.raises(WellFormedError, match="`[+]` needs Int and Int, got Account and Account"):
        check_module(parse_module("f a:Account b:Account\nbody\n  audit.emit (Foo (a + b))\n"))


@pytest.mark.parametrize("body, msg", [
    ("ledger.credit a (b - b)", "needs Int and Int, got Account and Account"),
    ("ledger.credit n a", "argument 1 of `ledger.credit` must be Account, got Int"),
    ("need a > b else Bad", "needs Int and Int, got Account and Account"),
    ("need ledger.owner a = n else Bad", "compares Owner with Int"),
    ("need n else Bad", "must be Bool, got Int"),
    ("ledger.balance a", "value of type Int is computed and discarded"),
    ("audit.emit n", "argument 1 of `audit.emit` must be Event, got Int"),
    ("x = ledger.credit a n", "has no value (Unit)"),
])
def test_f7_type_errors_are_rejected(body, msg):
    with pytest.raises(WellFormedError, match=re.escape(msg)):
        check_module(parse_module(f"f a:Account b:Account n:Int\nbody\n  {body}\n"))


@gap("Record/replay of handler calls is not implemented (roadmap 4).")
def test_f7_effects_are_replayable():
    assert hasattr(Interp, "replay")


# =====================================================================================
# F8. The ratchet loop: counterexamples -> examples, survivors -> guarantees
# =====================================================================================

def test_f8_counterexample_is_a_valid_eg_line_that_replays():
    defs, reps = analysed(open("examples/ledger_v1.k").read())
    cx = [g for g in reps["transfer"]["guarantees"] if g["status"] == "refuted"][0]["counterexample"]
    ex = parse_example(cx["eg"][3:], None)
    assert cx["confirmed_by_interpreter"] and check.run_examples(defs, dataclasses.replace(
        defs["transfer"], examples=[ex]))[0]["pass"]


def test_f8_ill_formed_mutants_are_not_counted_as_kills():              # fixed (step 3)
    defs = parse_module(V3)
    m = check.mutation_score(defs, defs["transfer"])
    # 12 well-formed body mutants + 1 payload mutant (step 6); the ill-formed one is listed, not counted
    assert m["total"] == 13 and m["ill_formed"] == ["drop `have = ledger.balance from`"]


def test_f8_failure_payloads_are_mutated():                             # fixed (step 6)
    d = parse_module(V4)["transfer"]
    assert any("Insufficient" in desc and "`-` → `+`" in desc for desc, _ in check.mutants(d))


def test_f8_surviving_mutants_point_at_the_missing_guarantee():
    """The ratchet's survivor signal works: v3's survivors are exactly the audit mutants, and
    adding `emit` (v4) kills them."""
    for src, want in ((V3, 3), (V4, 0)):
        defs = parse_module(src)
        surv = check.mutation_score(defs, defs["transfer"])["survived"]
        assert len(surv) == want and all("audit.emit" in s for s in surv)


# =====================================================================================
# F9. Tokens shape syntax, but what gets loaded matters more
# =====================================================================================

needs_tok = pytest.mark.skipif(views._TOK is None, reason="tokenizer not installed")


@needs_tok
@pytest.mark.parametrize("word", "need ensure keep uses fails eg why else body emit".split())
def test_f9_keywords_are_one_token(word):
    assert views.tokens(f"  {word} x") - views.tokens("  x") == 1


@needs_tok
def test_f9_editor_view_is_constant_as_the_module_grows():
    sizes = []
    for k in (1, 4):
        src = "\n\n".join(re.sub(r"\b(transfer|payout|settle|deposit|withdraw|rebalance|month_end)\b",
                                 lambda m: f"{m.group(1)}{i}", LARGE) for i in range(k))
        defs, reps = analysed(src)
        sizes.append((views.tokens(src), views.tokens(views.editor(reps, "month_end0", no_whys(defs)))))
    (m1, e1), (m4, e4) = sizes
    assert e1 == e4 and m4 > 3.9 * m1


@needs_tok
def test_f9_editor_view_beats_reading_the_call_chain_only_from_depth_two(large):
    defs, reps = large
    blocks = {m.group(1): m.group(0).strip() for m in re.finditer(r"(?ms)^([a-z]\w*) .*?(?=^[a-z]\w* |\Z)", LARGE)}

    def chain(n, acc):
        if n not in acc:
            acc.append(n)
            for c in callees(defs, defs[n]):
                chain(c, acc)
        return acc
    ratio = {n: views.tokens(views.editor(reps, n, no_whys(defs))) /
             views.tokens("\n\n".join(blocks[x] for x in chain(n, []))) for n in defs}
    assert ratio["month_end"] < 0.45 and ratio["payout"] < 0.8
    assert ratio["transfer"] > 1.0          # leaf: inferred lines are added and nothing is skipped


# =====================================================================================
# Incidental (not a fundamental, found during the audit)
# =====================================================================================

def test_incidental_and_after_comparison_parses():                      # fixed (step 1)
    d = parse_module("f a:Account amount:Int\n  need amount > 0 and amount < 10 else Bad\nbody\n"
                     "  ledger.credit a amount\n")["f"]
    assert d.needs[0][1] == ("bin", "and", parse_expr("amount > 0"), parse_expr("amount < 10"))


@pytest.mark.parametrize("src", ["a > 0 and b < 10", "a > 0 or b = c and d != e", "(a or b) and c",
                                 "a and (b or c)", "a + 1 > b - 2 and c"])
def test_incidental_boolean_clauses_print_and_reparse(src):
    e = parse_expr(src)
    assert parse_expr(code(e)) == e


def test_incidental_why_for_unknown_name_is_a_clean_error(capsys):      # fixed (step 10)
    from kern.__main__ import main
    assert main(["why", "examples/ledger_v4.k", "nosuch", "--why", "examples/ledger_v4.why"]) == 2
    assert "no definition named `nosuch`" in capsys.readouterr().err
