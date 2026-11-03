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

from kern import check, render, views
from kern import run as runmod
from kern import why as whymod
from kern.infer import callees, fails, uses
from kern.prove import Prover
from kern.report import analyse
from kern.run import Interp, KernError, World
from kern.syntax import ParseError, code, parse_example, parse_expr, parse_module

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


@gap("A body-level `need` (transfer's `have >= amount else Insufficient`) is behaviour "
     "callers can hit but cannot read: the spec view lists Insufficient in `fails` but "
     "not when it happens.")
def test_f1_spec_view_says_when_each_failure_happens():
    defs, reps = analysed(V4)
    text = views.spec(reps["transfer"], NO_WHY)
    for n in defs["transfer"].needs + [s for s in defs["transfer"].body if s[0] == "need"]:
        assert code(n[1]) in text, f"failure {n[2]}: condition `{code(n[1])}` not in spec view"


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
                                           ("do", parse_expr("audit.emit (Junk clock.now)"))])
    assert uses(defs, garbage) == uses(defs, t)
    after = statuses({"guarantees": Prover(dict(defs, transfer=garbage)).prove(defs["payout"])[0]})
    assert before == after == ["proven"] * 3


def test_f2_hidden_callee_failure_rolls_back_whole_call():
    """payout's 2nd transfer fails on a body-level need its spec does not mention."""
    defs = parse_module(V4)
    w = World({"A": 100, "B": 0, "H": 0}, {"A": "u1", "B": "u1", "H": "u1"})
    snap = copy.deepcopy(w)
    out, after = Interp(defs).run("payout", ["A", "B", "H", 100, 5], w)
    assert out == ("Insufficient", [5])
    assert after.bal == snap.bal and after.audit == snap.audit


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


@gap("Soundness: the prover evaluates `ensure` in the body's env, so a `let` that shadows a "
     "parameter changes what the guarantee means. Body credits amount+1; page says ✔ and the "
     "badge is green (random tests never reach amount > 2000).")
def test_f2_shadowed_parameter_cannot_fake_a_proof():
    defs, reps = analysed(SHADOW)
    assert statuses(reps["sneaky"]) != ["proven"]


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


@gap("Soundness: a recursive call is verified by assuming the definition's own spec, with no "
     "termination measure, so a non-terminating body proves any ensure.")
def test_f2_self_recursion_does_not_prove_by_assumption():
    defs = parse_module(RECURSIVE)
    assert statuses({"guarantees": Prover(defs).prove(defs["loop"])[0]}) != ["proven"]


@gap("Mutual recursion crashes `check` (RecursionError in the interpreter, uncaught by "
     "proptest) instead of being reported.")
def test_f2_mutual_recursion_is_reported_not_crashed():
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


@gap("Atomicity holds only for declared failures (Fail). A runtime error (unknown account, "
     "type error) mid-body leaves the world half-mutated.")
def test_f2_runtime_error_mid_body_is_atomic():
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


@gap("No frame conditions: a body that also moves money between two other accounts gets "
     "'All guarantees proven'. A reviewer who reads only guarantees never sees it.")
def test_f3_guarantees_reveal_extra_writes():
    defs, reps = analysed(STEAL)
    assert render._status(reps["transfer"])[0] != "ok"


@gap("Nothing compares guarantees between builds: deleting every balance ensure and the keep "
     "leaves the definition green with no warning.")
def test_f3_removed_guarantees_are_surfaced():
    weak = V4.replace("  ensure ledger.balance' to = ledger.balance to + amount\n", "") \
             .replace("  ensure ledger.balance' from = ledger.balance from - amount\n", "") \
             .replace("  keep ledger.total\n  emit TransferDone from to", "  emit TransferDone from to")
    _, reps = analysed(weak)
    assert render._status(reps["transfer"])[0] != "ok"


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


@gap("`fails` is not the full set of ways a call can end: an unknown account raises "
     "KernError, which is not a declared failure and is not inferred.")
def test_f4_every_outcome_is_ok_or_an_inferred_failure():
    defs = parse_module(V4)
    w = World({"A": 500}, {"A": "u1"})
    out, _ = Interp(defs).run("transfer", ["A", "Z", 10], w)
    assert out == ("ok",) or out[0] in fails(defs, defs["transfer"])


@gap("Spec clauses are not checked for purity: `ensure ledger.debit a 5` parses and then "
     "crashes the prover (Z3Exception) instead of being rejected.")
def test_f4_effectful_spec_is_rejected_cleanly():
    with pytest.raises(ParseError):
        analysed("g a:Account\n  ensure ledger.debit a 5\nbody\n  ledger.credit a 0\n")


# =====================================================================================
# F5. Evidence over assertion: the page claims no more than the checks established
# =====================================================================================

def test_f5_no_guarantees_is_not_labelled_all_proven():          # fixed in this audit
    _, reps = analysed("nospec a:Account amount:Int\nbody\n  ledger.debit a amount\n")
    assert render._status(reps["nospec"]) == ("warn", "No guarantees stated")


def test_f5_prover_timeout_is_labelled_as_such(monkeypatch):      # fixed in this audit
    class NoAnswer(z3.Solver):
        def check(self, *a):
            return z3.unknown
    monkeypatch.setattr(z3, "Solver", NoAnswer)
    _, reps = analysed(V4)
    assert set(statuses(reps["transfer"])) == {"unknown"}
    assert render._status(reps["transfer"]) == ("warn", "Not proven: prover gave no answer")
    assert not reps["transfer"]["clean"] and reps["transfer"]["mutation"] is None


@gap("Vacuity: contradictory needs make every ensure 'proven' (no input ever succeeds). The "
     "page shows 0/500 random successes but still says 'All guarantees proven'.")
def test_f5_vacuous_spec_is_flagged():
    _, reps = analysed("never a:Account amount:Int\n  need amount > 0 else A\n  need amount < 0 else B\n"
                       "  ensure ledger.balance' a = ledger.balance a + 999\nbody\n  ledger.debit a amount\n")
    assert render._status(reps["never"])[0] != "ok"


@gap("A correct definition is shown ✗ 'Broken guarantees': without frame conditions, "
     "withdraw's spec says nothing about `to`, so rebalance's ensure is 'refuted' with a "
     "counterexample that does not even succeed in the interpreter.")
def test_f5_unconfirmed_refutation_is_not_shown_as_broken(large):
    _, reps = large
    g = reps["rebalance"]["guarantees"][1]
    assert not g["counterexample"]["confirmed_by_interpreter"]   # the evidence
    assert g["status"] != "refuted"


def test_f5_unconfirmed_refutation_says_so(large):                # fixed in this audit
    defs, reps = large
    assert "not reproduced in the interpreter" in render.terminal(reps, no_whys(defs))
    assert "Not reproduced in the interpreter." in render.card(reps["rebalance"], NO_WHY)


def test_f5_inherited_failures_are_attributed_to_the_right_callee(large):   # fixed in this audit
    defs, reps = large
    html_ = render.card(reps["rebalance"], NO_WHY)
    rows = {c: re.search(rf"<code>{c}</code> refuses.*?</tr>", html_).group(0) for c in ("withdraw", "deposit")}
    assert "BadAmount" in rows["withdraw"] and "Insufficient" in rows["withdraw"]
    assert "BadAmount" in rows["deposit"] and "Insufficient" not in rows["deposit"]


@pytest.mark.parametrize("src", ["ledger.balance a - (amount - fee)", "a - (b + c)", "(a = b) = (c = d)",
                                 "x + (y - 1) - (z - (w + 2))", "ledger.balance' a = ledger.balance a + (b - b)"])
def test_f5_printed_clauses_mean_what_was_written(src):              # fixed in this audit
    e = parse_expr(src)
    assert parse_expr(code(e)) == e


@gap("Honesty tiers ◐ tested-only and ○ runtime-checked-only are not implemented.")
def test_f5_tested_only_and_runtime_only_tiers_exist():
    assert "◐" in render.ICON.values() and "○" in render.ICON.values()


@gap("Spec strength mostly measures the examples: v4 transfer with every `keep`/`ensure` "
     "deleted still scores 13/13, so the proven guarantees contribute nothing measurable.")
def test_f5_spec_strength_drops_when_guarantees_are_deleted():
    weak = V4.replace("  ensure ledger.balance' to = ledger.balance to + amount\n", "") \
             .replace("  ensure ledger.balance' from = ledger.balance from - amount\n", "") \
             .replace("  keep ledger.total\n  emit TransferDone from to", "  emit TransferDone from to")
    _, full = analysed(V4, mutation=True)
    _, gone = analysed(weak, mutation=True)
    assert gone["transfer"]["mutation"]["killed"] < full["transfer"]["mutation"]["killed"]


# =====================================================================================
# F6. Separate knowledge by role: why is lazy, hash-anchored, never obligatory
# =====================================================================================

def test_f6_body_change_marks_why_stale():
    store = whymod.load("examples/ledger_v4.why")
    d = parse_module(V4.replace("  ledger.credit to amount\n  audit", "  ledger.credit to amount\n"
                                "  ledger.credit to 0\n  audit"))["transfer"]
    assert whymod.status(store, d)["stale"]


def test_f6_eg_only_change_does_not_mark_why_stale():
    """Decision to confirm: examples are evidence, not semantics, so why stays fresh."""
    store = whymod.load("examples/ledger_v4.why")
    d = parse_module(V4.replace("transfer A B 100 -> ok | A:400 B:100", "transfer A B 7 -> ok | A:493 B:7"))
    assert not whymod.status(store, d["transfer"])["stale"]


@gap("Content addressing: Def.hash excludes `eg`, so two definitions with different examples "
     "share a hash. A hash-keyed store (roadmap 2) would merge them.")
def test_f6_hash_distinguishes_definitions_with_different_examples():
    a = parse_module(V4)["transfer"]
    b = parse_module(V4.replace("transfer A B 100 -> ok | A:400 B:100", "transfer A B 7 -> ok | A:493 B:7"))["transfer"]
    assert a.hash != b.hash


@gap("A callee change that alters the caller's inferred interface (payout gains SameAccount "
     "in v2) does not change the caller's hash, so its why is not flagged. README claimed "
     "it was.")
def test_f6_callee_interface_change_marks_caller_why_stale():
    v1, v2 = parse_module(open("examples/ledger_v1.k").read()), parse_module(open("examples/ledger_v2.k").read())
    assert fails(v1, v1["payout"]) != fails(v2, v2["payout"])         # the interface did change
    assert v1["payout"].hash != v2["payout"].hash


@gap("Orphaned why entries (definition renamed or deleted) are silently dropped from every "
     "view, and `anchor` keeps them without a warning.")
def test_f6_orphaned_why_entries_are_reported():
    store = whymod.load("examples/ledger_v4.why")
    defs = parse_module(V4.replace("transfer", "move"))
    shown = sum(whymod.status(store, d)["count"] for d in defs.values())
    assert shown == sum(len(s["entries"]) for s in store.values())


@gap("Rule 3 ('nothing that must be obeyed lives only in prose') is not checked: a why entry "
     "phrased as an obligation is accepted without a flag.")
def test_f6_obligation_phrased_why_is_flagged():
    store = whymod.load("examples/ledger_v4.why")
    d = parse_module(V4)["transfer"]
    st = whymod.status(store, d)
    assert any("must" in e for e in st["entries"])                     # the input
    assert st.get("obligations")


# =====================================================================================
# F7. Closed world of effects: everything goes through handlers
# =====================================================================================

@pytest.mark.parametrize("body", ["__import__ 1", "os.system 1", "from.__class__", "print 1"])
def test_f7_body_cannot_reach_python(body):
    defs = parse_module(f"f from:Account to:Account\nbody\n  {body}\n")
    with pytest.raises(KernError):
        Interp(defs).run("f", ["A", "B"], World({"A": 1, "B": 2}, {"A": "u", "B": "u"}))


@gap("Unknown names are not a static error: `uses` silently ignores `os.system`, and the "
     "prover crashes with ValueError instead of a check error.")
def test_f7_unknown_names_are_rejected_before_proving():
    with pytest.raises(ParseError):
        analysed("f from:Account to:Account\nbody\n  os.system 1\n")


@gap("Values are Python values: `from + to` on two Accounts is string concatenation "
     "('AB') at runtime but integer addition in the prover.")
def test_f7_values_do_not_leak_python_semantics():
    defs = parse_module("f a:Account b:Account\nbody\n  audit.emit (Foo (a + b))\n")
    with pytest.raises(KernError):
        Interp(defs).run("f", ["A", "B"], World({"A": 1, "B": 2}, {"A": "u", "B": "u"}))


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


@gap("An ill-formed mutant (drop `have = ...` leaves an unbound name) is counted as a kill. "
     "In v3 it is 1 of 10 kills; excluding it gives 9/12.")
def test_f8_ill_formed_mutants_are_not_counted_as_kills():
    defs = parse_module(V3)
    m = check.mutation_score(defs, defs["transfer"])
    assert m["total"] == 12


@gap("The mutant generator never touches a failure payload (`Insufficient (amount - have)`), "
     "so spec strength says nothing about payloads.")
def test_f8_failure_payloads_are_mutated():
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

@gap("Parser: `and`/`or` bind looser than comparisons only on the right; "
     "`need amount > 0 and amount < 10 else Bad` is a parse error.")
def test_incidental_and_after_comparison_parses():
    parse_module("f a:Account amount:Int\n  need amount > 0 and amount < 10 else Bad\nbody\n  ledger.credit a amount\n")
