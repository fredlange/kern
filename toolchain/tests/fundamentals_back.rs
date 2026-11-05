//! Adversarial tests against docs/FUNDAMENTALS.md (port of tests/test_fundamentals.py, the prover and checker half):
//! F1 (views load the minimum), F2 (verification licenses not reading), F3 (the spec is the artifact, incl. `ok
//! otherwise`), F5 (evidence over assertion), F8 (the ratchet loop) and the Clock section. The front-end sections
//! are kern-9b's port (fundamentals_front.rs).

mod common;
use common::ROOT;

use indexmap::IndexMap;
use kern::check::{mutants, mutation_score, proptest, run_examples};
use kern::infer::{callees, lifted, uses, writes};
use kern::prove::Prover;
use kern::render;
use kern::report::{analyse, Report, Status};
use kern::run::{Interp, Outcome, RunError, Value, World};
use kern::syntax::{code, parse_example, parse_expr, parse_module, Def, Module, Need, Outcome as EgOutcome, Stmt};
use kern::views;
use kern::why;
use regex::Regex;

fn read(p: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/{p}")).unwrap()
}

fn v1() -> String {
    read("examples/ledger-tour/ledger_v1.k")
}
fn v3() -> String {
    read("examples/ledger-tour/ledger_v3.k")
}
fn v4() -> String {
    read("examples/ledger-tour/ledger_v4.k")
}
fn v5() -> String {
    read("examples/ledger-tour/ledger_v5.k")
}
fn v6() -> String {
    read("examples/ledger-tour/ledger_v6.k")
}
fn v7() -> String {
    read("examples/ledger-tour/ledger_v7.k")
}
fn large() -> String {
    read("examples/ledger-large/ledger_large.k")
}

fn parse(src: &str) -> Module {
    parse_module(src, true).unwrap()
}

fn analysed(src: &str, mutation: bool) -> (Module, IndexMap<String, Report>) {
    let defs = parse(src);
    let reps = analyse(&defs, mutation, None, None).unwrap();
    (defs, reps)
}

/// The well-formedness error analysing `src` gives (Python: pytest.raises(WellFormedError)).
fn wf_error(src: &str) -> String {
    let defs = parse(src);
    match analyse(&defs, false, None, None) {
        Err(e) => e.message(),
        Ok(_) => panic!("expected a WellFormedError"),
    }
}

fn statuses(r: &Report) -> Vec<&'static str> {
    r.guarantees.iter().map(|g| g.status.as_str()).collect()
}

fn no_why() -> why::Status {
    why::Status::default()
}

fn no_whys(defs: &Module) -> IndexMap<String, why::Status> {
    defs.defs.keys().map(|n| (n.clone(), why::Status::default())).collect()
}

fn world(accts: &[(&str, i128, &str)], clock: i128) -> World {
    World::new(
        accts.iter().map(|(a, b, _)| (a.to_string(), *b)).collect(),
        accts.iter().map(|(a, _, o)| (a.to_string(), o.to_string())).collect(),
        clock,
    )
}

fn s(x: &str) -> Value {
    Value::Str(x.into())
}
fn i(n: i128) -> Value {
    Value::Int(n)
}

fn replace_body(d: &Def, body: Vec<Stmt>) -> Def {
    let mut m = d.clone();
    m.body = body;
    m
}

fn expr(src: &str) -> kern::syntax::Expr {
    parse_expr(src, None).unwrap()
}

// =====================================================================================
// F1. Optimise for the reader: views load the minimum, never a callee body
// =====================================================================================

#[test]
fn f1_editor_view_never_contains_a_callee_body() {
    let (defs, reps) = analysed(&large(), false);
    let whys = no_whys(&defs);
    for (name, d) in &defs.defs {
        let text = views::editor(&reps, name, &whys);
        let own: Vec<&String> = d.src_lines.get("body").map(|b| b.iter().collect()).unwrap_or_default();
        for c in callees(&defs, d) {
            for line in defs.defs[c.as_str()].src_lines.get("body").cloned().unwrap_or_default() {
                assert!(own.contains(&&line) || !text.contains(&line), "{name}'s editor view leaks {c}'s body: {line}");
            }
        }
    }
}

#[test]
fn f1_spec_view_says_when_each_failure_happens() {
    // fixed (step 8). Was: Insufficient appeared in `fails` but its condition lived only in the body
    let (defs, reps) = analysed(&v4(), false);
    let text = views::spec(&reps["transfer"], &no_why());
    assert!(text.contains("body need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)"), "{text}");
    for n in &defs.defs["transfer"].needs {
        let Need::Need(c, _, _) = n else { panic!() };
        assert!(text.contains(&code(c)));
    }
}

#[test]
fn f1_lifting_stops_at_the_first_write() {
    // payout's second transfer needs `balance from >= fee` *after* the first transfer debited from, so it cannot
    // be stated in payout's pre-state and must not be lifted. Owner checks read nothing ever written.
    let defs = parse(&v4());
    let got: Vec<String> = lifted(&defs, &defs.defs["payout"]).iter().map(|l| code(&l.cond)).collect();
    assert!(got.contains(&"ledger.balance from >= amount".to_string()) && !got.contains(&"ledger.balance from >= fee".to_string()));
    for w in ["ledger.owner from = ledger.owner house", "fee > 0", "from != house"] {
        assert!(got.contains(&w.to_string()), "{w} not in {got:?}");
    }
}

#[test]
fn f1_modular_proof_assumes_lifted_conditions() {
    // settle's spec says nothing about from != to, but the mutant `payout to to ...` can never succeed (transfer
    // refuses SameAccount). With lifted conditions the prover sees that.
    let defs = parse(&large());
    let d = &defs.defs["settle"];
    let mut body = vec![Stmt::Do(expr("payout to to house amount fee"))];
    body.extend(d.body[1..].iter().cloned());
    let md = replace_body(d, body);
    let mdefs = defs.with_def(md.clone());
    let got: std::collections::BTreeSet<&str> = Prover::new(&mdefs).prove(&md, false).0.iter().map(|g| g.status.as_str()).collect();
    assert_eq!(got, ["vacuous"].into());
}

// =====================================================================================
// F2. Verification licenses not reading: modular proofs, callee specs only
// =====================================================================================

#[test]
fn f2_caller_proof_does_not_read_callee_body() {
    let defs = parse(&v4());
    let before: Vec<Status> = Prover::new(&defs).prove(&defs.defs["payout"], false).0.iter().map(|g| g.status).collect();
    let t = &defs.defs["transfer"];
    // same inferred interface (uses), completely different behaviour
    let mut body = vec![
        Stmt::Let("have".into(), expr("ledger.balance from")),
        Stmt::Need(expr("have >= amount"), "Insufficient".into(), vec![expr("amount - have")]),
    ];
    body.extend([
        Stmt::Do(expr("ledger.credit from 999")),
        Stmt::Do(expr("ledger.debit to 1")),
        Stmt::Do(expr("audit.emit (Junk clock.now)")),
    ]);
    let garbage = replace_body(t, body);
    // the full inferred interface a modular call reads: uses, writes and lifted refusal conditions
    assert_eq!(uses(&defs, &garbage), uses(&defs, t));
    assert_eq!(writes(&defs, &garbage), writes(&defs, t));
    let lc = |ls: Vec<kern::infer::Lifted>| ls.into_iter().map(|l| (code(&l.cond), l.ctor, l.origin)).collect::<Vec<_>>();
    assert_eq!(lc(lifted(&defs, &garbage)), lc(lifted(&defs, t)));
    let gdefs = defs.with_def(garbage);
    let after: Vec<Status> = Prover::new(&gdefs).prove(&defs.defs["payout"], false).0.iter().map(|g| g.status).collect();
    assert_eq!(before, after);
    assert_eq!(after, vec![Status::Proven; 3]);
}

#[test]
fn f2_hidden_callee_failure_rolls_back_whole_call() {
    // payout's 2nd transfer fails on a body-level need its spec does not mention
    let defs = parse(&v4());
    let mut w = world(&[("A", 100, "u1"), ("B", 0, "u1"), ("H", 0, "u1")], 0);
    let snap = w.clone();
    let out = Interp::new(&defs).run("payout", &[s("A"), s("B"), s("H"), i(100), i(5)], &mut w).unwrap();
    assert_eq!(out, Outcome::Fail("Insufficient".into(), vec![i(5)]));
    // the caller's own world is rolled back (fixed, step 2); time passes for refused calls too (clock step)
    assert_eq!(w.bal, snap.bal);
    assert_eq!(w.audit.len(), snap.audit.len());
    assert_eq!(w.clock, snap.clock + 1);
}

#[test]
fn f2_modular_proof_agrees_with_execution_on_hidden_failures() {
    let defs = parse(&v4());
    let stats = proptest(&defs, &defs.defs["payout"], 2000, 11).unwrap();
    assert!(stats.failures.get("Insufficient").copied().unwrap_or(0) > 0);
    assert!(stats.violations.is_empty());
}

const SHADOW: &str = "
sneaky to:Account amount:Int
  need amount > 2000 else Small
  ensure ledger.balance' to = ledger.balance to + amount
body
  amount = amount + 1
  ledger.credit to amount
";

#[test]
fn f2_shadowed_parameter_cannot_fake_a_proof() {
    // fixed (step 3). Was: ✔ and a green badge while the body credits amount+1. Now rejected before proving
    assert!(wf_error(SHADOW).contains("`amount` shadows an existing name"));
}

#[test]
fn f2_spec_cannot_read_body_lets() {
    let src = v4().replace("  keep ledger.total\n  ensure ledger.balance' from", "  keep have\n  ensure ledger.balance' from");
    assert!(wf_error(&src).contains("unknown name `have`"));
}

#[test]
fn f2_shadowed_parameter_really_violates_the_spec() {
    // companion: the interpreter confirms the ✔ above would be wrong
    let defs = parse(SHADOW);
    let mut pre = world(&[("A", 0, "u1")], 0);
    let mut post = pre.clone();
    let mut it = Interp::new(&defs);
    let out = it.run("sneaky", &[s("A"), i(3000)], &mut post).unwrap();
    assert_eq!(out, Outcome::Ok);
    assert_eq!(post.bal["A"], 3001);
    assert!(!it.holds(&defs.defs["sneaky"], &[s("A"), i(3000)], &mut pre, &mut post).unwrap().is_empty());
}

const RECURSIVE: &str = "
loop a:Account
  ensure ledger.balance' a = ledger.balance a + 1000
body
  loop a
";

#[test]
fn f2_self_recursion_does_not_prove_by_assumption() {
    // fixed (step 3). Was: proven by assuming its own spec. Now rejected until there are termination measures
    assert!(wf_error(RECURSIVE).contains("recursive (loop -> loop)"));
}

#[test]
fn f2_mutual_recursion_is_reported_not_crashed() {
    let src = "
ping a:Account
  ensure ledger.balance' a = ledger.balance a + 7
body
  pong a
pong a:Account
  ensure ledger.balance' a = ledger.balance a + 7
body
  ping a
";
    assert!(wf_error(src).contains("recursive (ping -> pong -> ping)"));
}

#[test]
fn f2_runtime_error_mid_body_is_atomic() {
    let defs = parse("t from:Account to:Account amount:Int\nbody\n  ledger.debit from amount\n  ledger.credit to amount\n");
    let mut w = world(&[("A", 100, "u1")], 0);
    let r = Interp::new(&defs).run("t", &[s("A"), s("Z"), i(10)], &mut w);
    assert!(matches!(r, Err(RunError::Kern(_))), "{r:?}");
    assert_eq!(w.bal.len(), 1);
    assert_eq!(w.bal["A"], 100);
}

// =====================================================================================
// F3. The spec is the primary artifact: review guarantees, not diffs
// =====================================================================================

const STEAL: &str = "
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
";

#[test]
fn f3_guarantees_reveal_extra_writes() {
    // fixed (step 7). The guarantees are all proven (they are true), but the inferred write set and the gap show
    let (defs, reps) = analysed(STEAL, false);
    let r = &reps["transfer"];
    let want: std::collections::BTreeSet<String> = ["from", "to", "x", "y"].iter().map(|x| x.to_string()).collect();
    assert_eq!(r.writes, want);
    assert_eq!(views::unguaranteed_writes(r), vec!["x".to_string(), "y".to_string()]);
    assert!(views::iface(r, &no_why()).contains("writes from to x y"));
    assert!(render::card(r, &defs, &no_why()).contains("No guarantee says what the new balance of <code>x</code>, <code>y</code> is."));
}

#[test]
fn f3_inferred_frame_proves_a_two_callee_definition() {
    // rebalance was shown ✗ (then ?) because withdraw's spec said nothing about `to`
    let (_, reps) = analysed(&large(), false);
    assert_eq!(statuses(&reps["rebalance"]), ["proven", "proven"]);
    assert!(reps["rebalance"].clean);
}

#[test]
fn f3_write_inference_through_callees_and_lets() {
    let defs = parse(&(large() + "\nalias a:Account b:Account n:Int\nbody\n  x = a\n  ledger.credit x n\n  transfer b a n\n"));
    let set = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(writes(&defs, &defs.defs["alias"]), set(&["a", "b"]));
    assert_eq!(writes(&defs, &defs.defs["month_end"]), set(&["from", "to", "house"]));
}

fn weak_v4() -> String {
    v4().replace("  ensure ledger.balance' to = ledger.balance to + amount\n", "")
        .replace("  ensure ledger.balance' from = ledger.balance from - amount\n", "")
        .replace("  keep ledger.total\n  emit TransferDone from to", "  emit TransferDone from to")
}

#[test]
fn f3_removed_guarantees_are_surfaced() {
    // fixed (step 6)
    let dir = common::tmp("f3_removed");
    let base = dir.join("ledger.strength").to_string_lossy().to_string();
    let (vdefs, vreps) = analysed(&v4(), true);
    kern::baseline::write(&base, &vreps, &vdefs).unwrap();
    let defs = parse(&weak_v4());
    let b = kern::baseline::load(&base);
    let reps = analyse(&defs, true, Some(&b), None).unwrap();
    assert_eq!(render::status(&reps["transfer"]), ("warn", "Weaker than baseline".to_string()));
    let msg = reps["transfer"].regression.join(" ");
    assert!(msg.contains("keep ledger.total") && msg.contains("guarantee strength fell from 9/14 to 3/14"), "{msg}");
    // payout's text did not change, but its proof rested on transfer's deleted ensures
    assert_eq!(reps["payout"].regression, vec!["was clean at the baseline, is not now".to_string()]);
}

// ---- F3, continued: `ok otherwise` lets a spec say *when* a call succeeds -----------

fn ok_goal(r: &Report) -> &kern::report::Guarantee {
    r.guarantees.iter().find(|g| g.kind == "ok").expect("an ok goal")
}

#[test]
fn f3_ok_otherwise_is_proven_through_a_modular_call() {
    let (_, reps) = analysed(&v7(), false);
    assert_eq!(ok_goal(&reps["transfer"]).status, Status::Proven);
    assert_eq!(ok_goal(&reps["payout"]).status, Status::Proven);
    assert!(reps["transfer"].clean && reps["payout"].clean);
}

#[test]
fn f3_ok_otherwise_refutes_an_unstated_refusal() {
    // v6 payout says `fee >= 0`, but transfer refuses a zero fee
    let (_, reps) = analysed(&v6(), false);
    let g = ok_goal(&reps["payout"]);
    let cx = g.counterexample.as_ref().unwrap();
    assert!(g.status == Status::Refuted && cx.confirmed_by_interpreter);
    let ex = parse_example(&cx.eg[3..], None).unwrap();
    assert!(matches!(&ex.outcome, EgOutcome::Fail(c, _) if c == "BadAmount"), "{:?}", ex.outcome);
    assert_eq!(ex.call.1.last(), Some(&kern::syntax::Key::Int(0))); // the zero fee
    assert!(reps["payout"].props.violations.iter().any(|v| matches!(v, kern::report::Violation::Clause { clause, .. } if clause == "ok otherwise")));
}

#[test]
fn f3_ok_otherwise_needs_callees_that_say_when_they_refuse() {
    // same rule as `emit`: a claim about a callee's refusals needs the callee's spec to make one
    let v7s = v7();
    let restated = v7s.split_once("payout").unwrap().1.split_once("keep").unwrap().0.split_once('\n').unwrap().1;
    let src = v5().replace("  need fee >= 0 else BadAmount\n", restated);
    let (_, reps) = analysed(&src, false);
    let g = ok_goal(&reps["payout"]);
    assert_eq!(g.status, Status::Unknown);
    assert!(g.reason.as_deref().unwrap().contains("transfer, whose spec does not say when it refuses"));
}

#[test]
fn f3_ok_otherwise_is_never_promoted_to_tested_only() {
    // ◐ means "held in N random successes"; that says nothing about refusals
    let (_, reps) = analysed(
        "
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
",
        false,
    );
    assert!(reps["wrap"].props.succeeded >= 50); // promotion would apply
    let g = ok_goal(&reps["wrap"]);
    assert_eq!(g.status, Status::Unknown);
    assert!(g.reason.as_deref().unwrap().contains("loose, whose spec does not say when it refuses"));
}

#[test]
fn f3_ok_otherwise_catches_a_body_need_that_can_fire() {
    // a body need that may refuse where the spec needs pass is a broken claim
    for cond in ["have > amount", "ledger.balance to >= amount"] {
        let src = v7().replacen(
            "body\n  ledger.debit from amount\n",
            &format!("body\n  have = ledger.balance from\n  need {cond} else Insufficient 0\n  ledger.debit from amount\n"),
            1,
        );
        let (_, reps) = analysed(&src, false);
        let g = ok_goal(&reps["transfer"]);
        assert!(g.status == Status::Refuted && g.counterexample.as_ref().unwrap().confirmed_by_interpreter, "{cond}");
        assert_eq!(reps["payout"].guarantees[0].status, Status::Conditional, "{cond}"); // trust propagates
    }
}

#[test]
fn f3_ok_otherwise_redundant_body_need_is_allowed() {
    let src = v7().replacen(
        "body\n  ledger.debit from amount\n",
        "body\n  have = ledger.balance from\n  need have >= amount else Insufficient 0\n  ledger.debit from amount\n",
        1,
    );
    assert_eq!(ok_goal(&analysed(&src, false).1["transfer"]).status, Status::Proven);
}

#[test]
fn f3_ok_otherwise_mutants_that_add_refusals_are_caught() {
    // with the redundant body need kept, the obligation itself kills `>=` → `>` and `from` → `to`. The two
    // survivors (drop it, flip its payload) are equivalent: the spec guard runs first
    let src = v7().replacen(
        "body\n  ledger.debit from amount\n",
        "body\n  have = ledger.balance from\n  need have >= amount else Insufficient (amount - have)\n  ledger.debit from amount\n",
        1,
    );
    let m = analysed(&src, true).1["transfer"].mutation.clone().unwrap();
    assert_eq!((m.guarantee_killed, m.total), (12, 14));
    let mut missed: Vec<String> = m.guarantee_missed.iter().map(|(d, _)| d.clone()).collect();
    missed.sort();
    assert_eq!(
        missed,
        [
            "`-` → `+` in payload of `need have >= amount else Insufficient (amount - have)`",
            "drop `need have >= amount else Insufficient (amount - have)`"
        ]
    );
}

#[test]
fn f3_ok_otherwise_is_opt_in_and_hashed() {
    // without the marker nothing changes (hashes included); with it, the hash changes
    let d5 = parse(&v5());
    let hashes: Vec<(String, String)> = d5.defs.iter().map(|(n, d)| (n.clone(), d.hash())).collect();
    assert_eq!(hashes, vec![("transfer".to_string(), "683e3393".to_string()), ("payout".to_string(), "9b00a7f7".to_string())]);
    let (plain, marked) = (parse(&v7().replace("  ok otherwise\n", "")), parse(&v7()));
    assert_ne!(plain.defs["transfer"].hash(), marked.defs["transfer"].hash());
    assert!(analysed(&v5(), false).1.values().all(|r| r.guarantees.iter().all(|g| g.kind != "ok")));
    assert!(parse_module(&v7().replacen("  ok otherwise\n", "  ok sometimes\n", 1), true).is_err());
}

#[test]
fn f3_ok_otherwise_page_and_views() {
    let (defs, reps) = analysed(&v7(), false);
    let card = render::card(&reps["payout"], &defs, &no_why());
    assert!(card.contains("refuses for no other reason") && !card.contains("(from transfer)"));
    assert!(views::spec(&reps["payout"], &no_why()).contains("ok otherwise"));
    let (d6, r6) = analysed(&v6(), false);
    assert!(render::card(&r6["payout"], &d6, &no_why()).contains("(from transfer)")); // unproven: keep showing them
}

// =====================================================================================
// F5. Evidence over assertion: the page claims no more than the checks established
// =====================================================================================

#[test]
fn f5_no_guarantees_is_not_labelled_all_proven() {
    let (_, reps) = analysed("nospec a:Account amount:Int\nbody\n  ledger.debit a amount\n", false);
    assert_eq!(render::status(&reps["nospec"]), ("warn", "No guarantees stated".to_string()));
}

// test_f5_prover_timeout_with_enough_random_successes_is_tested_only and test_f5_prover_timeout_without_test_
// support_is_unknown are in validation/tests/prover_no_answer.rs: they need every solver to answer "unknown"
// (Python monkeypatched z3.Solver), which the `faults` feature's KERN_FAULT=unknown does in its own process.

#[test]
fn f5_vacuous_spec_is_flagged() {
    // fixed (step 5)
    let (_, reps) = analysed(
        "never a:Account amount:Int\n  need amount > 0 else A\n  need amount < 0 else B\n  ensure ledger.balance' a = ledger.balance a + 999\nbody\n  ledger.debit a amount\n",
        false,
    );
    assert_eq!(render::status(&reps["never"]), ("fail", "Never succeeds".to_string()));
    assert_eq!(statuses(&reps["never"]), ["vacuous"]);
    assert!(reps["never"].guarantees[0].reason.as_deref().unwrap().contains("its own needs"));
}

#[test]
fn f5_vacuity_from_a_callee_spec_is_attributed_to_the_callee() {
    let (_, reps) = analysed(
        "
liar a:Account
  ensure ledger.balance' a = ledger.balance a + 1
  ensure ledger.balance' a = ledger.balance a + 2
body
  ledger.credit a 1
user a:Account
  ensure ledger.balance' a = ledger.balance a + 100
body
  liar a
",
        false,
    );
    let g = &reps["user"].guarantees[0];
    // 2026-11-04 (FZ finding 3): vacuity resting on a broken callee is `!`, blocked by it, with the reason kept
    assert_eq!(g.status, Status::Conditional);
    assert!(g.vacuity.as_deref().unwrap().contains("specs of liar"));
    assert_eq!(g.blocked_by, Some(vec!["liar".to_string()]));
    assert_eq!(statuses(&reps["liar"]), ["proven", "refuted"]); // the callee is where the bug is
}

const WEAK_CALLEE: &str = "
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
";

#[test]
fn f5_unconfirmed_refutation_is_not_shown_as_broken() {
    // fixed (step 5). `take`'s spec says nothing about ledger.total, so `move`'s keep is refuted from specs
    // alone, but `move` really does keep the total: the counterexample doesn't replay
    let (_, reps) = analysed(WEAK_CALLEE, false);
    let g = &reps["move"].guarantees[0];
    assert!(!g.counterexample.as_ref().unwrap().confirmed_by_interpreter); // the evidence
    assert_eq!(g.status, Status::Unconfirmed);
    assert_eq!(render::status(&reps["move"]), ("warn", "Not proven: counterexample not reproduced".to_string()));
    assert!(reps["move"].props.violations.is_empty());
}

#[test]
fn f5_unconfirmed_refutation_says_so() {
    // fixed in the audit
    let (defs, reps) = analysed(WEAK_CALLEE, false);
    assert!(render::terminal(&reps, &defs, &no_whys(&defs), &render::Orphans::new()).contains("not reproduced in the interpreter"));
    assert!(render::card(&reps["move"], &defs, &no_why()).contains("Not reproduced in the interpreter, so this is not evidence of a bug"));
}

#[test]
fn f5_inherited_failures_are_attributed_to_the_right_callee() {
    // fixed in the audit; since step 8 inherited failures are shown with their condition and origin
    let (defs, reps) = analysed(&large(), false);
    let card = render::card(&reps["rebalance"], &defs, &no_why());
    let row = Regex::new(r"(?s)<tr>(.*?)</tr>").unwrap();
    let tag = Regex::new(r"<[^>]+>").unwrap();
    let text: Vec<String> = row.captures_iter(&card).map(|c| tag.replace_all(&c[1], "").to_string()).collect();
    assert!(text.iter().any(|t| t == "balance of from &lt; amount→ Insufficient (from withdraw)"), "{text:?}"); // `<` was unescaped
    assert!(!text.iter().any(|t| t.contains("Insufficient") && t.contains("deposit")));
}

#[test]
fn f5_printed_clauses_mean_what_was_written() {
    // fixed in this audit
    for src in [
        "ledger.balance a - (amount - fee)",
        "a - (b + c)",
        "(a = b) = (c = d)",
        "x + (y - 1) - (z - (w + 2))",
        "ledger.balance' a = ledger.balance a + (b - b)",
    ] {
        let e = expr(src);
        assert_eq!(expr(&code(&e)), e, "{src}");
    }
}

#[test]
fn f5_tested_only_tier_exists() {
    // fixed (step 5)
    assert_eq!(render::icon(Status::Tested), "◐");
}

const ALL_STATUSES: [Status; 8] = [
    Status::Proven,
    Status::Assumed,
    Status::Conditional,
    Status::Refuted,
    Status::Unconfirmed,
    Status::Vacuous,
    Status::Unknown,
    Status::Tested,
];

#[test]
#[should_panic]
fn f5_runtime_only_tier_exists() {
    // gap (strict xfail in Python): tier ○ runtime-checked-only is not implemented (needs runtime checks, roadmap 4)
    assert!(ALL_STATUSES.iter().any(|s| render::icon(*s) == "○"));
}

#[test]
fn f5_spec_strength_drops_when_guarantees_are_deleted() {
    // fixed (step 6). Was: 13/13 either way, because examples killed everything first
    let full = analysed(&v4(), true).1["transfer"].mutation.clone().unwrap();
    let gone = analysed(&weak_v4(), true).1["transfer"].mutation.clone().unwrap();
    assert_eq!((full.guarantee_killed, gone.guarantee_killed), (9, 3)); // 3 = the audit mutants `emit` still catches
    assert_eq!((full.killed, gone.killed), (13, 13)); // the any-layer number cannot tell them apart
}

// =====================================================================================
// F8. The ratchet loop: counterexamples -> examples, survivors -> guarantees
// =====================================================================================

#[test]
fn f8_counterexample_is_a_valid_eg_line_that_replays() {
    let (defs, reps) = analysed(&v1(), false);
    let cx = reps["transfer"].guarantees.iter().find(|g| g.status == Status::Refuted).unwrap().counterexample.clone().unwrap();
    let ex = parse_example(&cx.eg[3..], None).unwrap();
    let mut d = defs.defs["transfer"].clone();
    d.examples = vec![ex];
    assert!(cx.confirmed_by_interpreter && run_examples(&defs, &d).unwrap()[0].pass);
}

#[test]
fn f8_ill_formed_mutants_are_not_counted_as_kills() {
    // fixed (step 3). 12 well-formed body mutants + 1 payload (step 6) + 1 timestamp (clock); ill-formed: listed
    let defs = parse(&v3());
    let m = mutation_score(&defs, &defs.defs["transfer"], 150);
    assert_eq!(m.total, 14);
    assert_eq!(m.ill_formed, vec!["drop `have = ledger.balance from`".to_string()]);
}

#[test]
fn f8_failure_payloads_are_mutated() {
    // fixed (step 6)
    let defs = parse(&v4());
    assert!(mutants(&defs.defs["transfer"]).iter().any(|(desc, _)| desc.contains("Insufficient") && desc.contains("`-` → `+`")));
}

#[test]
fn f8_surviving_mutants_point_at_the_missing_guarantee() {
    // v3's survivors are exactly the audit mutants; `emit` with `_` (v4) leaves only the timestamp one;
    // `emit … clock.now` kills that too
    for (src, want) in [(v3(), 4), (v4(), 1), (timed(), 0)] {
        let defs = parse(&src);
        let surv = mutation_score(&defs, &defs.defs["transfer"], 150).survived;
        assert_eq!(surv.len(), want, "{surv:?}");
        assert!(surv.iter().all(|s| s.contains("audit.emit")), "{surv:?}");
    }
}

// =====================================================================================
// Clock: one time per top-level call (docs/research/FIXES.md, "Clock")
// =====================================================================================

fn timed() -> String {
    v4().replacen(
        "  emit TransferDone from to amount _\n  eg A:500@u1 B:0@u1 | transfer",
        "  emit TransferDone from to amount clock.now\n  eg A:500@u1 B:0@u1 | transfer",
        1,
    )
}

#[test]
fn clock_spec_pins_the_timestamp() {
    let (_, reps) = analysed(&timed(), false);
    assert_eq!(statuses(&reps["transfer"]), ["proven"; 4]);
    let bad = timed().replace("audit.emit (TransferDone from to amount clock.now)", "audit.emit (TransferDone from to amount 0)");
    let (_, reps) = analysed(&bad, false);
    let g = reps["transfer"].guarantees.iter().find(|g| g.kind == "emit").unwrap();
    assert!(g.status == Status::Refuted && g.counterexample.as_ref().unwrap().confirmed_by_interpreter);
}

#[test]
fn clock_deadline_is_lifted_to_callers() {
    let defs = parse(
        "
pay to:Account amount:Int expires:Int
  ensure ledger.balance' to = ledger.balance to + amount
body
  need clock.now <= expires else Expired
  ledger.credit to amount
twice to:Account amount:Int expires:Int
body
  pay to amount expires
  pay to amount expires
",
    );
    let got: Vec<(String, String)> = lifted(&defs, &defs.defs["twice"]).into_iter().map(|l| (code(&l.cond), l.ctor)).collect();
    assert!(got.contains(&("clock.now <= expires".to_string(), "Expired".to_string())), "{got:?}");
}

#[test]
fn clock_timestamp_mutant_survives_underscore_but_not_clock_now() {
    let (v4d, timedd) = (parse(&v4()), parse(&timed()));
    let m4 = mutation_score(&v4d, &v4d.defs["transfer"], 150);
    let m5 = mutation_score(&timedd, &timedd.defs["transfer"], 150);
    assert!(m4.guarantee_missed.iter().any(|(s, _)| s.contains("clock.now") && s.contains("→ `0`")));
    assert!(!m5.guarantee_missed.iter().any(|(s, _)| s.contains("clock.now")));
}

fn last_arg(ev: &Value) -> Value {
    match ev {
        Value::Ctor(_, args) => args.last().cloned().unwrap(),
        other => panic!("not an event: {other:?}"),
    }
}

#[test]
fn clock_is_one_value_per_call_and_advances_between_calls() {
    let defs = parse(&v4());
    let mut w = world(&[("A", 500, "u1"), ("B", 0, "u1"), ("H", 0, "u1")], 7);
    let out = Interp::new(&defs).run("payout", &[s("A"), s("B"), s("H"), i(100), i(5)], &mut w).unwrap();
    assert_eq!(out, Outcome::Ok);
    assert_eq!(w.audit.iter().map(|e| last_arg(e).str()).collect::<Vec<_>>(), ["7", "7"]);
    assert_eq!(w.clock, 8);
    Interp::new(&defs).run("transfer", &[s("A"), s("B"), i(10)], &mut w).unwrap();
    assert_eq!(last_arg(w.audit.last().unwrap()).str(), "8");
    assert_eq!(w.clock, 9);
}

#[test]
fn clock_has_no_after_state() {
    let defs = parse("f a:Account\n  ensure clock.now' = clock.now\nbody\n  ledger.credit a 0\n");
    let e = kern::wf::check_module(&defs).unwrap_err().message();
    assert!(e.contains("the time is the same before and after"), "{e}");
}

#[test]
fn clock_counterexample_carries_the_time_and_replays() {
    // a refutation that depends on the time must say which time, or its eg line would not replay
    let (_, reps) = analysed(
        "
late to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to + amount
body
  need clock.now < 5 else Late
  ledger.credit to amount
  ledger.credit to (clock.now - clock.now)
  x = clock.now
  need x < 3 else Later
  ledger.credit to (3 - 3)
",
        false,
    );
    assert_eq!(statuses(&reps["late"]), ["proven"]); // the time is one value: x = clock.now < 3 holds
    let src = timed().replace("audit.emit (TransferDone from to amount clock.now)", "audit.emit (TransferDone from to amount 7)");
    let (_, reps) = analysed(&src, false);
    let cx = reps["transfer"].guarantees.iter().find(|g| g.kind == "emit").unwrap().counterexample.clone().unwrap();
    let ex = parse_example(&cx.eg[3..], None).unwrap();
    assert!(cx.confirmed_by_interpreter && ex.now != 7, "{}", cx.eg); // replays at a time other than 7
}
