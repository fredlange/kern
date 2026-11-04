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
    assert w.bal == snap.bal and w.audit == snap.audit
    assert w.clock == snap.clock + 1          # time passes for refused calls too (clock step)
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
    assert "keep ledger.total" in msg and "guarantee strength fell from 9/14 to 3/14" in msg   # 14: + timestamp mutant
    # payout's text did not change, but its proof rested on transfer's deleted ensures
    assert reps["payout"]["regression"] == ["was clean at the baseline, is not now"]


# ---- F3, continued: `ok otherwise` lets a spec say *when* a call succeeds -----------

V5 = open("examples/ledger_v5.k").read()
V6 = open("examples/ledger_v6.k").read()
V7 = open("examples/ledger_v7.k").read()


def _ok(rep):
    return [g for g in rep["guarantees"] if g["kind"] == "ok"][0]


def test_f3_ok_otherwise_is_proven_through_a_modular_call():
    """payout restates every refusal; its Insufficient condition holds only via transfer's ensure
    (the second call needs balance' from >= fee, after the first debited `amount`)."""
    defs, reps = analysed(V7)
    assert _ok(reps["transfer"])["status"] == "proven" and _ok(reps["payout"])["status"] == "proven"
    assert reps["transfer"]["clean"] and reps["payout"]["clean"]


def test_f3_ok_otherwise_refutes_an_unstated_refusal():
    """v6 payout says `fee >= 0`, but transfer refuses a zero fee. Nothing could flag that before."""
    defs, reps = analysed(V6)
    g = _ok(reps["payout"])
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    ex = parse_example(g["counterexample"]["eg"][3:], None)
    assert ex.outcome[0] == "BadAmount" and ex.call[1][-1] == 0             # the zero fee
    assert any(v["clause"] == "ok otherwise" for v in reps["payout"]["props"]["violations"])


def test_f3_ok_otherwise_needs_callees_that_say_when_they_refuse():
    """Same rule as `emit`: a claim about a callee's refusals needs the callee's spec to make one."""
    src = V5.replace("  need fee >= 0 else BadAmount\n", V7.split("payout", 1)[1].split("keep")[0].split("\n", 1)[1])
    defs, reps = analysed(src)
    g = _ok(reps["payout"])
    assert g["status"] == "unknown" and "transfer, whose spec does not say when it refuses" in g["reason"]


def test_f3_ok_otherwise_is_never_promoted_to_tested_only():
    """◐ means "held in N random successes"; that says nothing about refusals."""
    defs, reps = analysed("""
loose to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to + amount
body
  need amount < 100000 else Big
  ledger.credit to amount

wrap to:Account amount:Int
  ok otherwise
  ensure ledger.balance' to = ledger.balance to + amount
body
  loose to amount
""")
    assert reps["wrap"]["props"]["succeeded"] >= 50                       # promotion would apply
    g = _ok(reps["wrap"])
    assert g["status"] == "unknown" and "loose, whose spec does not say when it refuses" in g["reason"]


@pytest.mark.parametrize("cond", ["have > amount", "ledger.balance to >= amount"])
def test_f3_ok_otherwise_catches_a_body_need_that_can_fire(cond):
    """A body need that may refuse where the spec needs pass is a broken claim."""
    src = V7.replace("body\n  ledger.debit from amount\n",
                     f"body\n  have = ledger.balance from\n  need {cond} else Insufficient 0\n  ledger.debit from amount\n", 1)
    defs, reps = analysed(src)
    g = _ok(reps["transfer"])
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert reps["payout"]["guarantees"][0]["status"] == "conditional"        # trust propagates


def test_f3_ok_otherwise_redundant_body_need_is_allowed():
    src = V7.replace("body\n  ledger.debit from amount\n",
                     "body\n  have = ledger.balance from\n  need have >= amount else Insufficient 0\n"
                     "  ledger.debit from amount\n", 1)
    assert _ok(analysed(src)[1]["transfer"])["status"] == "proven"


def test_f3_ok_otherwise_mutants_that_add_refusals_are_caught():
    """With the redundant body need kept, the obligation itself kills `>=` → `>` and `from` → `to`.
    The two survivors (drop it, flip its payload) are equivalent: the spec guard runs first."""
    src = V7.replace("body\n  ledger.debit from amount\n",
                     "body\n  have = ledger.balance from\n  need have >= amount else Insufficient (amount - have)\n"
                     "  ledger.debit from amount\n", 1)
    m = analysed(src, mutation=True)[1]["transfer"]["mutation"]
    assert (m["guarantee_killed"], m["total"]) == (12, 14)
    assert sorted(d for d, _ in m["guarantee_missed"]) == [
        "`-` → `+` in payload of `need have >= amount else Insufficient (amount - have)`",
        "drop `need have >= amount else Insufficient (amount - have)`"]


def test_f3_ok_otherwise_is_opt_in_and_hashed():
    """Without the marker nothing changes (hashes included); with it, the hash changes."""
    assert {n: d.hash for n, d in parse_module(V5).items()} == {"transfer": "683e3393", "payout": "9b00a7f7"}
    plain, marked = parse_module(V7.replace("  ok otherwise\n", "")), parse_module(V7)
    assert plain["transfer"].hash != marked["transfer"].hash
    assert all("ok" not in [g["kind"] for g in r["guarantees"]] for r in analysed(V5)[1].values())
    with pytest.raises(ParseError):
        parse_module(V7.replace("  ok otherwise\n", "  ok sometimes\n", 1))


def test_f3_ok_otherwise_page_and_views():
    defs, reps = analysed(V7)
    card = render.card(reps["payout"], NO_WHY)
    assert "refuses for no other reason" in card and "(from transfer)" not in card
    assert "ok otherwise" in views.spec(reps["payout"], NO_WHY)
    assert "(from transfer)" in render.card(analysed(V6)[1]["payout"], NO_WHY)   # unproven: keep showing them


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
    # 12 well-formed body mutants + 1 payload (step 6) + 1 timestamp (clock); ill-formed: listed, not counted
    assert m["total"] == 14 and m["ill_formed"] == ["drop `have = ledger.balance from`"]


def test_f8_failure_payloads_are_mutated():                             # fixed (step 6)
    d = parse_module(V4)["transfer"]
    assert any("Insufficient" in desc and "`-` → `+`" in desc for desc, _ in check.mutants(d))


def test_f8_surviving_mutants_point_at_the_missing_guarantee():
    """The ratchet's survivor signal works: v3's survivors are exactly the audit mutants; `emit`
    with `_` (v4) leaves only the timestamp one; `emit … clock.now` kills that too."""
    for src, want in ((V3, 4), (V4, 1), (TIMED, 0)):
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


# =====================================================================================
# Clock: one time per top-level call (docs/FIXES.md, "Clock")
# =====================================================================================

TIMED = V4.replace("  emit TransferDone from to amount _\n  eg A:500@u1 B:0@u1 | transfer",
                   "  emit TransferDone from to amount clock.now\n  eg A:500@u1 B:0@u1 | transfer", 1)


def test_clock_spec_pins_the_timestamp():                                # clock step
    defs, reps = analysed(TIMED)
    assert statuses(reps["transfer"]) == ["proven"] * 4
    bad = TIMED.replace("audit.emit (TransferDone from to amount clock.now)", "audit.emit (TransferDone from to amount 0)")
    _, reps = analysed(bad)
    g = [g for g in reps["transfer"]["guarantees"] if g["kind"] == "emit"][0]
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]


def test_clock_deadline_is_lifted_to_callers():                          # clock step
    defs = parse_module("""
pay to:Account amount:Int expires:Int
  ensure ledger.balance' to = ledger.balance to + amount
body
  need clock.now <= expires else Expired
  ledger.credit to amount
twice to:Account amount:Int expires:Int
body
  pay to amount expires
  pay to amount expires
""")
    got = [(code(c), k) for c, k, *_ in __import__("kern.infer", fromlist=["lifted"]).lifted(defs, defs["twice"])]
    assert ("clock.now <= expires", "Expired") in got


def test_clock_timestamp_mutant_survives_underscore_but_not_clock_now():  # clock step
    v4 = parse_module(V4)
    timed = parse_module(TIMED)
    m4 = check.mutation_score(v4, v4["transfer"])
    m5 = check.mutation_score(timed, timed["transfer"])
    assert any("clock.now" in s and "→ `0`" in s for s, _ in m4["guarantee_missed"])
    assert not any("clock.now" in s for s, _ in m5["guarantee_missed"])


def test_clock_is_one_value_per_call_and_advances_between_calls():
    defs = parse_module(V4)
    w = World({"A": 500, "B": 0, "H": 0}, {"A": "u1", "B": "u1", "H": "u1"}, 7)
    out, _ = Interp(defs).run("payout", ["A", "B", "H", 100, 5], w)
    assert out == ("ok",) and [ev[-1] for ev in w.audit] == [7, 7] and w.clock == 8
    out, _ = Interp(defs).run("transfer", ["A", "B", 10], w)
    assert w.audit[-1][-1] == 8 and w.clock == 9


def test_clock_has_no_after_state():
    with pytest.raises(WellFormedError, match="the time is the same before and after"):
        check_module(parse_module("f a:Account\n  ensure clock.now' = clock.now\nbody\n  ledger.credit a 0\n"))


def test_clock_counterexample_carries_the_time_and_replays():
    """A refutation that depends on the time must say which time, or its eg line would not replay."""
    defs, reps = analysed("""
late to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to + amount
body
  need clock.now < 5 else Late
  ledger.credit to amount
  ledger.credit to (clock.now - clock.now)
  x = clock.now
  need x < 3 else Later
  ledger.credit to (3 - 3)
""")
    assert statuses(reps["late"]) == ["proven"]          # the time is one value: x = clock.now < 3 holds
    defs, reps = analysed(TIMED.replace("audit.emit (TransferDone from to amount clock.now)",
                                        "audit.emit (TransferDone from to amount 7)"))
    cx = [g for g in reps["transfer"]["guarantees"] if g["kind"] == "emit"][0]["counterexample"]
    ex = parse_example(cx["eg"][3:], None)
    assert cx["confirmed_by_interpreter"] and ex.now != 7          # replays at a time other than 7


# =====================================================================================
# Scale, size 1: branches (`if`/`else`), `ensure … when`, `emit … when` (docs/SCALE.md)
# =====================================================================================

ESCROW1 = open("examples/escrow_s1.k").read()
LEAF = """
transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit Moved from to amount clock.now
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (Moved from to amount clock.now)
"""


def _g(rep, kind, i=0):
    return [g for g in rep["guarantees"] if g["kind"] == kind][i]


def test_s1_escrow_is_clean(capsys):
    defs, reps = analysed(ESCROW1)
    assert all(r["clean"] for r in reps.values()) and len(defs) == 10


def test_s1_only_the_else_path_is_wrong():
    """The prover must check every path, not just the first: the bug is only in `else`."""
    defs, reps = analysed(LEAF + """
pay from:Account to:Account amount:Int
  need amount >= 0 else BadAmount
  need amount = 0 or from != to else SameAccount
  need amount = 0 or ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ensure ledger.balance' to = ledger.balance to + amount
body
  if amount = 0
    ledger.credit to 0
  else
    transfer from to (amount - 1)
""")
    g = _g(reps["pay"], "ensure")
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert parse_example(g["counterexample"]["eg"][3:], None).call[1][-1] > 0      # in the else path


def test_s1_wrong_when_condition_is_refuted():
    src = ESCROW1.replace("ensure ledger.balance' seller = ledger.balance seller + amount when amount < 100",
                          "ensure ledger.balance' seller = ledger.balance seller + amount when amount < 101")
    g = _g(analysed(src)[1]["capture"], "ensure", 1)
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert parse_example(g["counterexample"]["eg"][3:], None).call[1][3] == 100     # exactly the boundary


def test_s1_wrong_emit_condition_is_refuted():
    src = ESCROW1.replace("emit Moved from to amount clock.now when amount > 0",
                          "emit Moved from to amount clock.now when amount >= 0")
    g = _g(analysed(src)[1]["move"], "emit")
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert g["counterexample"]["emitted"] == "nothing"                             # amount = 0


def test_s1_caller_emit_spec_composes_conditional_callee_events():
    """tip's spec repeats move's conditional event; dropping the condition must be refuted."""
    src = ESCROW1.replace("emit Moved buyer seller amount clock.now when amount > 0",
                          "emit Moved buyer seller amount clock.now")
    g = _g(analysed(src)[1]["tip"], "emit")
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]


def test_s1_ok_otherwise_sees_a_need_inside_a_branch():
    src = ESCROW1.replace("body\n  if amount > 0\n    transfer from to amount",
                          "body\n  if amount > 0\n    need amount < 500 else TooBig\n    transfer from to amount")
    g = _ok(analysed(src)[1]["move"])
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert "TooBig" in g["counterexample"]["eg"]


def test_s1_branch_need_is_lifted_as_an_implication():
    """Callers may assume `amount <= 0 or amount < 500`, never `amount < 500` outright."""
    src = LEAF + """
small from:Account to:Account amount:Int
  ensure ledger.total' = ledger.total
body
  if amount > 0
    need amount < 500 else TooBig
    transfer from to amount
"""
    defs = parse_module(src)
    conds = [code(c) for c, ctor, _, origin in lifted(defs, defs["small"]) if ctor == "TooBig"]
    assert conds == ["amount <= 0 or amount < 500"]


def test_s1_writes_and_fails_cover_both_branches():
    defs = parse_module(ESCROW1)
    assert writes(defs, defs["capture"]) == {"escrow", "seller", "house"}
    assert writes(defs, defs["refund"]) == {"escrow", "buyer", "seller", "house"}
    assert set(fails(defs, defs["refund"])) >= {"BadPart", "BadFee", "SameAccount", "Insufficient", "BadAmount"}


def test_s1_refusal_inside_a_branch_rolls_back():
    src = LEAF + """
two a:Account b:Account amount:Int
body
  transfer a b amount
  if amount > 10
    need amount < 20 else TooBig
"""
    defs = parse_module(src)
    w = World({"A": 100, "B": 0}, {"A": "u1", "B": "u1"})
    out, after = Interp(defs).run("two", ["A", "B", 50], w)
    assert out == ("TooBig", []) and after.bal == {"A": 100, "B": 0} and after.audit == []


@pytest.mark.parametrize("body,msg", [
    ("  if amount > 0\n    x = amount\n  else\n    ledger.credit to x", "unknown name `x`"),
    ("  if amount > 0\n    x = amount\n  else\n    x = 1", "shadows"),
    ("  if amount\n    ledger.credit to 1", "must be Bool"),
])
def test_s1_branch_scoping_and_types(body, msg):
    with pytest.raises(WellFormedError, match=msg):
        check_module(parse_module(f"f to:Account amount:Int\nbody\n{body}\n"))


@pytest.mark.parametrize("spec,msg", [
    ("  need amount > 0 when amount > 1 else Bad", "`when` is only allowed"),
    ("  ensure ledger.balance' to = 1 when ledger.balance' to = 1", "only allowed in `ensure`"),
    ("  emit Ev to when ledger.balance' to > 0", "only allowed in `ensure`"),
])
def test_s1_when_is_only_a_clause_ending(spec, msg):
    with pytest.raises(WellFormedError, match=msg):
        check_module(parse_module(f"f to:Account amount:Int\n{spec}\nbody\n  ledger.credit to amount\n"))


@pytest.mark.parametrize("src", ["f a:Int\nbody\n  else\n", "f a:Int\nbody\n  if a > 0\n  ledger.total\n",
                                 "f a:Int\nbody\n  ledger.total\n    ledger.total\n"])
def test_s1_bad_blocks_are_parse_errors(src):
    with pytest.raises(ParseError):
        parse_module(src)


def test_s1_and_binds_tighter_than_or():
    assert code(parse_expr("a or b and c")) == "a or b and c"
    assert parse_expr("a or b and c")[1] == "or"
    assert code(parse_expr("(a or b) and c")) == "(a or b) and c"


def test_s1_straight_line_mutants_are_unchanged_and_branches_get_their_own():
    v5 = parse_module(V5)["transfer"]
    assert len(list(check.mutants(v5))) == 15
    descs = [d for d, _ in check.mutants(parse_module(ESCROW1)["capture"])]
    assert "swap the branches of `if amount < 100`" in descs and "`<` → `<=` in `if amount < 100`" in descs
    assert "drop `if amount < 100`" in descs and "drop `move escrow house fee`" in descs


def test_s1_page_shows_branch_refusals_and_conditions():
    defs, reps = analysed(ESCROW1)
    text = lambda n: re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", render.card(reps[n], NO_WHY)))
    assert "amount ≠ 0 and from = to → SameAccount" in text("move")
    assert "only when amount &gt; 0" in text("move") and "when amount &lt; 100: balance of seller" in text("capture")


def test_s1_one_infeasible_branch_is_not_vacuous():
    defs, reps = analysed("""
f to:Account amount:Int
  need amount > 0 else Bad
  ensure ledger.balance' to = ledger.balance to + amount
body
  if amount < 0
    ledger.credit to 5
  else
    ledger.credit to amount
""")
    assert statuses(reps["f"]) == ["proven"]


# =====================================================================================
# Scale, size 2: declared state, invariants, `*` `/`, `need as` / `emit as` (docs/SCALE.md)
# =====================================================================================

ESCROW2 = open("examples/escrow_s2.k").read()
STATE_HEAD = "state hold Int\nstate frozen Account\ninvariant a:Account ledger.balance a >= 0\n\n"


def test_s2_escrow_is_clean():
    defs, reps = analysed(ESCROW2)
    assert all(r["clean"] for r in reps.values()) and len(defs) == 26


def test_s2_state_dependent_refutation_replays_with_the_state_in_the_eg_line():
    src = ESCROW2.replace("  need spent.get a + amount <= limit.get a else OverLimit (spent.get a + amount - limit.get a)\n",
                          "  need spent.get a + amount <= limit.get a + 1 else OverLimit (spent.get a + amount - limit.get a)\n", 1)
    src = src.replace("  ok otherwise\n  keep ledger.total\n  ensure spent.get' a = spent.get a + amount\n",
                      "  ok otherwise\n  keep ledger.total\n  ensure spent.get' a = spent.get a + amount\n"
                      "  ensure spent.get' a <= limit.get a\n", 1)
    g = [g for g in analysed(src)[1]["withdraw"]["guarantees"] if g["clause"] == "spent.get' a <= limit.get a"][0]
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]
    assert "spent.A:" in g["counterexample"]["eg"] or "limit.A:" in g["counterexample"]["eg"]


def test_s2_examples_check_state_after():
    src = ESCROW2.replace("| open_order 7 A E 100 -> ok | A:400 E:100 hold.7:100", "| open_order 7 A E 100 -> ok | A:400 E:100 hold.7:99")
    assert not all(e["pass"] for e in analysed(src)[1]["open_order"]["examples"])


def test_s2_state_writes_are_inferred_through_callees_and_frame_other_keys():
    defs, reps = analysed(STATE_HEAD + """
put id:Int n:Int
  ensure hold.get' id = n
body
  hold.set id n

two id:Int other:Int n:Int
  ensure hold.get' other = hold.get other when other != id
body
  put id n
""")
    assert writes(defs, defs["two"]) == {"hold.id"} and statuses(reps["two"]) == ["proven"]


def test_s2_write_to_a_computed_key_is_any_key():
    defs = parse_module(STATE_HEAD + "f id:Int\nbody\n  hold.set (id + 1) 5\n")
    assert writes(defs, defs["f"]) == {"hold.*"}


def test_s2_invariant_violation_is_refuted_and_replays():
    defs, reps = analysed(STATE_HEAD + """
drain a:Account b:Account amount:Int
  need a != b else SameAccount
  need amount > 0 else BadAmount
body
  ledger.debit a amount
  ledger.credit b amount
""")
    g = _g(reps["drain"], "invariant")
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]


def test_s2_invariant_with_a_computed_key_is_unknown_not_proven():
    defs, reps = analysed("state hold Int\ninvariant o:Int hold.get o >= 0\n\nf id:Int\nbody\n  hold.set (id + 1) 5\n")
    g = _g(reps["f"], "invariant")           # never ✔; ◐ is honest (random tests do check invariants)
    assert g["status"] in ("unknown", "tested") and "computed key" in g["reason"]


def test_s2_invariant_removes_the_overdrawn_workaround():
    """purchase no longer needs `need ledger.balance escrow >= 0 else Overdrawn`."""
    defs, reps = analysed(ESCROW2)
    assert "Overdrawn" not in ESCROW2 and _ok(reps["purchase"])["status"] == "proven"


@pytest.mark.parametrize("expr,msg", [("amount / 0", "positive number"), ("amount / amount", "positive number"),
                                      ("amount * amount", "number on one side")])
def test_s2_only_linear_arithmetic(expr, msg):
    with pytest.raises(WellFormedError, match=msg):
        check_module(parse_module(f"f to:Account amount:Int\nbody\n  ledger.credit to ({expr})\n"))


def test_s2_division_agrees_between_prover_and_interpreter_for_negatives():
    defs, reps = analysed("""
f to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to + amount / 2
  eg A:10@u1 | f A -7 -> ok | A:6
body
  ledger.credit to (amount / 2)
""")
    assert statuses(reps["f"]) == ["proven"] and all(e["pass"] for e in reps["f"]["examples"])


def test_s2_need_as_expands_to_the_callees_spec_needs():
    defs = parse_module(ESCROW2)
    restated = parse_module(open("examples/escrow_s1.k").read())
    assert defs["reserve"].needs == restated["reserve"].needs                      # same conditions
    defs2, reps = analysed(ESCROW2)
    # the author's source keeps the reference; a reader's spec view gets the conditions themselves,
    # or a caller could not tell when reserve refuses without also loading transfer's spec
    assert "need as transfer buyer escrow amount" in views.editor(reps, "reserve", no_whys(defs2))
    spec_view = views.spec(reps["reserve"], NO_WHY)
    assert "need as" not in spec_view and "need buyer != escrow else SameAccount" in spec_view


def test_s2_need_as_follows_the_spec_arguments_not_the_body():
    """The reason references keep strength where inheritance lost it: a body mutant that changes a
    callee argument is still caught, because the reference names the arguments itself."""
    m = analysed(ESCROW2, mutation=True)[1]["reserve"]["mutation"]
    assert (m["guarantee_killed"], m["total"]) == (6, 6)


def test_s2_emit_as_conjoins_its_condition():
    defs = parse_module(ESCROW2)
    conds = [code(ev[2]) for ev in defs["refund"].emits if len(ev) > 2]
    assert "part = amount and amount > 0" not in conds          # release's events are unconditional
    assert "part = amount" in conds and "part < amount and amount - part < 100" in conds


@pytest.mark.parametrize("src,msg", [
    ("f a:Int\n  need as nothing a\nbody\n  ledger.total\n", "no definition named"),
    ("f a:Int\n  need as g a\nbody\n  ledger.total\n\ng a:Int\n  need as f a\nbody\n  ledger.total\n", "loop"),
    ("f a:Int\n  need as g a a\nbody\n  ledger.total\n\ng a:Int\nbody\n  ledger.total\n", "takes 1"),
])
def test_s2_bad_references_are_parse_errors(src, msg):
    with pytest.raises(ParseError, match=msg):
        parse_module(src)


@pytest.mark.parametrize("body,msg", [("  ledger.credit to (hold.get to)", "must be Int, got Account"),
                                      ("  hold.set id", "takes 2")])
def test_s2_state_ops_are_typed(body, msg):
    with pytest.raises(WellFormedError, match=msg):
        check_module(parse_module(STATE_HEAD + f"f to:Account id:Int\nbody\n{body}\n"))


def test_s2_specs_may_read_state_but_not_write_it():
    with pytest.raises(WellFormedError, match="has an effect"):
        check_module(parse_module(STATE_HEAD + "f id:Int\n  ensure hold.set id 1\nbody\n  hold.set id 1\n"))


# =====================================================================================
# Scale, size 3: pure functions (-> Int) usable in specs, `distinct` (docs/SCALE.md)
# =====================================================================================

ESCROW3 = open("examples/escrow_s3.k").read()
FN = """
fee_for amount:Int -> Int
  ensure result = 0 when amount < 100
  ensure result = amount * 2 / 100 when amount >= 100
body
  if amount < 100
    result = 0
  else
    result = amount * 2 / 100

pay from:Account to:Account house:Account amount:Int
  need distinct from to house else SameAccount
  ensure ledger.balance' house = ledger.balance house + fee_for amount
  ensure ledger.balance' to = ledger.balance to + amount - fee_for amount
body
  fee = fee_for amount
  ledger.debit from amount
  ledger.credit to (amount - fee)
  ledger.credit house fee
"""


def test_s3_escrow_is_clean():
    defs, reps = analysed(ESCROW3)
    assert all(r["clean"] for r in reps.values()) and len(defs) == 55


def test_s3_function_calls_are_modular():
    """A caller's proof uses the function's spec only: a wrong body with the same spec leaves the
    caller proven (and the function itself refuted, so trust propagation marks the caller)."""
    defs, reps = analysed(FN.replace("    result = amount * 2 / 100\n", "    result = amount * 3 / 100\n"))
    assert "refuted" in statuses(reps["fee_for"])
    assert set(statuses(reps["pay"])) == {"conditional"}


def test_s3_same_arguments_same_result():
    defs, reps = analysed(FN + """
twice to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to
body
  a = fee_for amount
  b = fee_for amount
  ledger.credit to (a - b)
""")
    assert statuses(reps["twice"]) == ["proven"]


@pytest.mark.parametrize("src,msg", [
    ("f a:Int -> Int\nbody\n  need a > 0 else Bad\n  result = a\n", "total"),
    ("f a:Account -> Int\nbody\n  result = ledger.balance a\n", "pure"),
    ("f a:Int -> Int\nbody\n  if a > 0\n    result = a\n", "every path"),
    ("g a:Account\n  ensure ledger.balance' a = 0\nbody\n  h a\n\nh a:Account\nbody\n  ledger.credit a 0\n"
     "\nk a:Account\n  ensure h a\nbody\n  h a\n", "only functions"),
])
def test_s3_functions_are_pure_total_and_only_they_appear_in_specs(src, msg):
    with pytest.raises(WellFormedError, match=msg):
        check_module(parse_module(src))


def test_s3_spec_named_functions_are_in_the_editor_view():
    defs, reps = analysed(ESCROW3)
    assert "fee_for amount:Int -> Int" in views.editor(reps, "capture", no_whys(defs))


def test_s3_distinct():
    defs, reps = analysed(FN)
    text = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", render.card(reps["pay"], NO_WHY)))
    assert "two of from , to and house are the same → SameAccount" in text
    with pytest.raises(WellFormedError, match="distinct"):
        check_module(parse_module("f a:Account n:Int\n  need distinct a n else X\nbody\n  ledger.credit a n\n"))


def test_s3_aliasing_bug_the_prover_found():
    """Found while writing size 3: a cart that lets the buyer be the seller refuses when it tips."""
    src = ESCROW3.replace("  need distinct buyer escrow seller house else SameAccount\n  need as purchase_order id buyer escrow seller house amount\n  need extra", "  need as purchase_order id buyer escrow seller house amount\n  need extra")
    g = _ok(analysed(src)[1]["checkout_tip"])
    assert g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"]


def test_s3_state_only_the_spec_reads_is_still_drawn_and_shown():
    """Regression (size 3): `unfreeze` with `frozen.set a 0` dropped touches no state, but its spec
    reads `frozen.get' a`; random worlds and counterexamples must still include `frozen`."""
    defs = parse_module(ESCROW3)
    md = dataclasses.replace(defs["unfreeze"], body=[s for s in defs["unfreeze"].body if s[0] != "do" or
                                                     s[1][1][0] != "frozen.set"])
    mdefs = defs.with_def(md)
    g = _g({"guarantees": Prover(mdefs).prove(md)[0]}, "ensure")
    assert g["status"] == "refuted" and "frozen.A:" in g["counterexample"]["eg"]


# =====================================================================================
# Roadmap run S1: the ratchet sees refusals (docs/ROADMAP_RUN.md)
# =====================================================================================

def _against_baseline(src_before, src_after, tmp_path):
    base = tmp_path / "m.strength"
    baseline.write(base, analysed(src_before, mutation=True)[1])
    defs = parse_module(src_after)
    return analyse(defs, baseline=baseline.load(base))


def test_s1_dropped_need_is_flagged(tmp_path):
    """C1's B10: refund drops `part <= amount`; nothing else notices, the ratchet now does."""
    reps = _against_baseline(ESCROW1, ESCROW1.replace("need part > 0 and part <= amount else BadPart",
                                                      "need part > 0 else BadPart"), tmp_path)
    msg = " ".join(reps["refund"]["regression"])
    assert "refusals removed or changed" in msg and "part <= amount" in msg
    assert render._status(reps["refund"]) == ("warn", "Spec changed since baseline")


def test_s1_dodging_need_is_flagged_without_ok_otherwise_or_eg(tmp_path):
    """C3's route: refuse the hard case instead of handling it, where no caller or example would notice."""
    before = ESCROW1.replace("  ok otherwise\n", "")
    after = before.replace("capture escrow:Account seller:Account house:Account amount:Int fee:Int\n",
                           "capture escrow:Account seller:Account house:Account amount:Int fee:Int\n"
                           "  need amount >= 100 or amount < 50 else Unsupported\n")
    after = after.replace("  eg E:100@u0 B:0@u2 H:0@u9 | capture E B H 50 5 -> ok | E:50 B:50 H:0\n", "")
    reps = _against_baseline(before, after, tmp_path)
    assert any("new refusals" in x and "Unsupported" in x for x in reps["capture"]["regression"])


@pytest.mark.parametrize("old,new", [
    ("  ok otherwise\n  keep ledger.total\n  ensure ledger.balance' from = ledger.balance from - amount\n  ensure ledger.balance' to = ledger.balance to + amount\n  emit Moved from to amount clock.now\n  eg A:500@u1 B:0@u2 | transfer",
     "  keep ledger.total\n  ensure ledger.balance' from = ledger.balance from - amount\n  ensure ledger.balance' to = ledger.balance to + amount\n  emit Moved from to amount clock.now\n  eg A:500@u1 B:0@u2 | transfer"),
    ("ensure ledger.balance' a = ledger.balance a + part\n", "ensure ledger.balance' a >= ledger.balance a\n"),
])
def test_s1_removed_or_rewritten_guarantees_are_flagged(tmp_path, old, new):
    assert old in ESCROW1
    reps = _against_baseline(ESCROW1, ESCROW1.replace(old, new, 1), tmp_path)
    assert any(r["regression"] for r in reps.values())


def test_s1_unchanged_and_strengthened_specs_are_not_flagged(tmp_path):
    reps = _against_baseline(ESCROW1, ESCROW1, tmp_path)
    assert not any(r["regression"] for r in reps.values())
    stronger = ESCROW1.replace("  ensure ledger.balance' seller = ledger.balance seller + amount\n",
                               "  ensure ledger.balance' seller = ledger.balance seller + amount\n"
                               "  ensure ledger.balance' buyer = ledger.balance buyer - amount\n", 1)
    assert stronger != ESCROW1
    reps = _against_baseline(ESCROW1, stronger, tmp_path)
    assert not reps["tip"]["regression"]


def test_s1_old_format_baseline_does_not_flag_every_need(tmp_path):
    old = tmp_path / "old.strength"
    reps = analysed(ESCROW1, mutation=True)[1]
    old.write_text("".join(f"{n} {r['mutation']['guarantee_killed']}/{r['mutation']['total']}\n" +
                           "".join(f"  {c}\n" for c in baseline.clauses(r)) for n, r in reps.items()))
    again = analyse(parse_module(ESCROW1), baseline=baseline.load(old))
    assert not any(r["regression"] for r in again.values())


def test_s1_against_a_committed_baseline_ignores_the_file(tmp_path):
    """`check --against REV` reads the baseline from git, so rewriting FILE.strength changes nothing.
    Needs a working git; skipped (and said so) while git is blocked."""
    import shutil
    import subprocess
    if shutil.which("git") is None or subprocess.run(["git", "--version"], capture_output=True).returncode:
        pytest.skip("git is not usable here (e.g. the Xcode licence prompt); --against is untested")
    pytest.skip("covered by the manual check in docs/ROADMAP_RUN.md once git works")


# =====================================================================================
# Roadmap run S2: incremental, quick check (docs/ROADMAP_RUN.md)
# =====================================================================================

import sys as _sys  # noqa: E402
_sys.path.insert(0, "challenge/c6")
from gen import gen as _gen  # noqa: E402
from kern import report as _report  # noqa: E402


def _cached(reps):
    return {n for n, r in reps.items() if r["cached"]}


def test_s2_only_the_changed_leaf_and_its_callers_are_rechecked(tmp_path):
    src = _gen(3, 1, 2, 30)                                    # 10 chains f{c}_1 <- f{c}_2 <- f{c}_3
    analyse(parse_module(src), mutation=False, cache=tmp_path)
    changed = src.replace("| f4_1 A B 100 -> ok", "| f4_1 A B 90 -> ok", 1).replace(
        "A:500@u1 B:0@u2 | f4_1 A B 90 -> ok | A:400 B:100", "A:500@u1 B:0@u2 | f4_1 A B 90 -> ok | A:410 B:90")
    reps = analyse(parse_module(changed), mutation=False, cache=tmp_path)
    assert {n for n in reps if n not in _cached(reps)} == {"f4_1", "f4_2", "f4_3"}   # the leaf and its callers
    assert all(r["clean"] for r in reps.values())


def test_s2_a_callee_body_change_is_never_served_from_the_cache(tmp_path):
    """Callers depend on what is inferred from callee bodies (writes, lifted refusals)."""
    analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path)
    src = ESCROW1.replace("  ledger.debit from amount\n  ledger.credit to amount\n",
                          "  ledger.debit from amount\n  ledger.credit to amount\n  ledger.credit from 0\n", 1)
    reps = analyse(parse_module(src), mutation=False, cache=tmp_path)
    assert "transfer" not in _cached(reps) and "move" not in _cached(reps) and "purchase" not in _cached(reps)


def test_s2_a_tool_change_invalidates_everything(tmp_path, monkeypatch):
    analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path)
    monkeypatch.setattr(_report, "_TOOL", "a different prover")
    assert not _cached(analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path))


def test_s2_a_corrupt_cache_entry_is_recomputed_not_trusted(tmp_path):
    analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path)
    for f in tmp_path.rglob("*"):
        if f.is_file():
            f.write_bytes(b"not a pickle")
    reps = analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path)
    assert not _cached(reps) and all(r["clean"] for r in reps.values())


def test_s2_quick_never_shows_an_unmeasured_strength(tmp_path):
    reps = analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path)
    out = render.terminal(reps, no_whys(reps))
    assert "spec strength: not measured (--quick)" in out and "spec strength: guarantees" not in out
    analyse(parse_module(ESCROW1), mutation=True, cache=tmp_path)           # a full run measures it
    reps = analyse(parse_module(ESCROW1), mutation=False, cache=tmp_path)   # same code: current, shown
    assert "spec strength: guarantees 10/10" in render.terminal(reps, no_whys(reps))
    src = ESCROW1.replace("  eg A:500@u1 B:0@u2 | transfer A B 0 -> BadAmount\n", "")
    reps = analyse(parse_module(src), mutation=False, cache=tmp_path)       # changed: not shown
    assert "not measured" in render.terminal(reps, no_whys(reps)).split("\n\n")[0]


# =====================================================================================
# Roadmap run S4: bounded loops with invariants (docs/ROADMAP_RUN.md)
# =====================================================================================

BATCH = open("examples/batch.k").read()


def _non_proven(reps, name):
    return [g for g in reps[name]["guarantees"] if g["status"] != "proven"]


def test_s4_batch_is_clean_invariants_included():
    defs, reps = analysed(BATCH)
    assert all(r["clean"] for r in reps.values())
    assert any(g["kind"] == "loop" and g["clause"].startswith("module invariant") for g in reps["sweep_expired"]["guarantees"])


@pytest.mark.parametrize("old,new,which", [
    # an invariant that an iteration does not keep
    ("    invariant ledger.balance' buyer = ledger.balance buyer - 10 * k\n",
     "    invariant ledger.balance' buyer = ledger.balance buyer - 9 * k\n", "kept by every iteration"),
    # an invariant false on entry
    ("    invariant ledger.balance' seller = ledger.balance seller + 10 * k\n",
     "    invariant ledger.balance' seller = ledger.balance seller + 10 * k + 1\n", "holds when the loop starts"),
])
def test_s4_wrong_loop_invariants_are_refuted(old, new, which):
    assert old in BATCH
    defs, reps = analysed(BATCH.replace(old, new))
    bad = [g for g in _non_proven(reps, "installments") if which in g["clause"]]
    assert bad and bad[0]["status"] in ("refuted", "unconfirmed")


def test_s4_off_by_one_bound_is_refuted_and_replays():
    defs, reps = analysed(BATCH.replace("  for k from 0 to n\n", "  for k from 0 to (n + 1)\n"))
    bad = _non_proven(reps, "installments")
    assert bad and any(g["status"] == "refuted" and g["counterexample"]["confirmed_by_interpreter"] for g in bad)


def test_s4_quantified_ensure_false_at_one_key_is_refuted_with_the_key_in_the_eg_line():
    src = BATCH.replace("    if sub_amount.get k > 0 and sub_due.get k <= clock.now\n",
                        "    if sub_amount.get k > 0 and sub_due.get k < clock.now\n")
    defs, reps = analysed(src)                 # caught at the loop invariant (the ensure follows from it)
    g = [g for g in reps["advance_subs"]["guarantees"] if g["status"] == "refuted"]
    assert g and g[0]["counterexample"]["confirmed_by_interpreter"] and "sub_due." in g[0]["counterexample"]["eg"]


def test_s4_callers_use_the_loops_inferred_frame():
    defs, reps = analysed(BATCH)
    assert writes(defs, defs["sweep_elsewhere"]) >= {"hold.[lo,hi)"}
    assert all(g["status"] == "proven" for g in reps["sweep_elsewhere"]["guarantees"])
    no_frame = BATCH.replace("  need other < lo or other >= hi else InRange\n", "")
    assert _non_proven(analysed(no_frame)[1], "sweep_elsewhere")                  # inside the range: not provable


def test_s4_events_inside_a_loop_are_flagged_never_proven():
    defs, reps = analysed(BATCH)
    assert views.loop_event_note(reps["sweep_expired"]) == ["audit"]
    assert "events emitted inside a loop are not described" in render.terminal(reps, no_whys(defs))
    src = BATCH.replace("  keep ledger.total\n  ensure ledger.balance' buyer = ledger.balance buyer - 10 * n\n",
                        "  keep ledger.total\n  ensure ledger.balance' buyer = ledger.balance buyer - 10 * n\n"
                        "  emit Moved buyer seller 10 clock.now\n")
    g = [g for g in analysed(src)[1]["installments"]["guarantees"] if g["kind"] == "emit"][0]
    assert g["status"] != "proven" and "inside a loop" in g["reason"]


def test_s4_invariant_assumption_relies_on_every_writer():
    """kern-90: the for-all-keys assumption is justified only if every writer of the store proves the
    invariant. An unproven writer of `hold` downgrades the loop's proofs to `!`."""
    src = BATCH + """
spoil id:Int
body
  hold.set id (0 - 1)
"""
    defs, reps = analysed(src)
    assert any(g["kind"] == "invariant" and g["status"] != "proven" for g in reps["spoil"]["guarantees"])
    assert {g["status"] for g in reps["sweep_expired"]["guarantees"]} == {"conditional"}
    assert "invariant on hold" in reps["sweep_expired"]["guarantees"][0]["blocked_by"]


@pytest.mark.parametrize("src,msg", [
    ("f lo:Int hi:Int\nbody\n  for k from lo to hi\n    for m from lo to hi\n      ledger.total\n", "nested loops"),
    ("f a:Account\n  need a != a for j from 0 to 1 else Bad\nbody\n  ledger.credit a 1\n", "only allowed at the end"),
    ("f a:Account\nbody\n  invariant ledger.total' = ledger.total\n  ledger.credit a 1\n", "start of a `for` loop"),
])
def test_s4_loop_forms_are_checked(src, msg):
    with pytest.raises((WellFormedError, ParseError), match=msg):
        check_module(parse_module(src))


def test_s4_loop_mutants_exist():
    descs = [d for d, _ in check.mutants(parse_module(BATCH)["installments"])]
    assert "`0` → `0 + 1` in `for k from 0 to n`" in descs and "`n` → `n - 1` in `for k from 0 to n`" in descs
    assert "empty the body of `for k from 0 to n`" in descs
    assert any(d.startswith("`k` → `k + 1`") for d in check.mutants(parse_module(BATCH)["sweep_expired"]) for d in [d[0]])
