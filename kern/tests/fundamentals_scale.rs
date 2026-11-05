//! Adversarial tests against docs/FUNDAMENTALS.md (port of tests/test_fundamentals.py, kern-8b's half of the scale and
//! roadmap sections): scale sizes 1-3 (branches, declared state and invariants, pure functions), roadmap S4 (loops),
//! S5 (assumed handlers), the probes and the FZ findings. The parse and type errors of these sections, the ratchet (S1)
//! and the cache (S2) are in fundamentals_front.rs; the tool-change cache test is in cache_tool_salt.rs.

mod common;
use common::ROOT;

use indexmap::IndexMap;
use kern::check::mutants;
use kern::host::{Host, Options};
use kern::infer::{fails, lifted, writes};
use kern::prove::Prover;
use kern::pyrandom::Random;
use kern::render;
use kern::report::{analyse, Guarantee, Report, Status};
use kern::run::{Interp, Outcome, RunError, Value, World};
use kern::syntax::{code, parse_example, parse_module, Emit, Expr, Key, Module, Stmt};
use kern::views;
use kern::why;
use regex::Regex;

fn read(p: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/{p}")).unwrap()
}

fn parse(src: &str) -> Module {
    parse_module(src, true).unwrap()
}

fn analysed(src: &str, mutation: bool) -> (Module, IndexMap<String, Report>) {
    let defs = parse(src);
    let reps = analyse(&defs, mutation, None, None).unwrap();
    (defs, reps)
}

fn no_why() -> why::Status {
    why::Status::default()
}

fn no_whys(defs: &Module) -> IndexMap<String, why::Status> {
    defs.defs.keys().map(|n| (n.clone(), why::Status::default())).collect()
}

fn statuses(r: &Report) -> Vec<&'static str> {
    r.guarantees.iter().map(|g| g.status.as_str()).collect()
}

/// Python `_g(rep, kind, i)`: the i-th guarantee of a kind.
fn g_kind<'r>(r: &'r Report, kind: &str, i: usize) -> &'r Guarantee {
    r.guarantees.iter().filter(|g| g.kind == kind).nth(i).unwrap_or_else(|| panic!("no {kind} #{i}"))
}

fn ok_goal(r: &Report) -> &Guarantee {
    g_kind(r, "ok", 0)
}

fn refuted_and_replays(g: &Guarantee) -> bool {
    g.status == Status::Refuted && g.counterexample.as_ref().is_some_and(|c| c.confirmed_by_interpreter)
}

fn eg_call(g: &Guarantee) -> (String, Vec<Key>) {
    let eg = &g.counterexample.as_ref().unwrap().eg;
    parse_example(&eg[3..], None).unwrap().call
}

fn int(k: &Key) -> i128 {
    match k {
        Key::Int(n) => *n,
        Key::Str(s) => panic!("not an Int: {s}"),
    }
}

/// The text of a proof-page card, tags stripped and whitespace collapsed.
fn card_text(r: &Report, defs: &Module) -> String {
    let tags = Regex::new(r"<[^>]+>").unwrap();
    let ws = Regex::new(r"\s+").unwrap();
    ws.replace_all(&tags.replace_all(&render::card(r, defs, &no_why()), " "), " ").to_string()
}

fn world(bal: &[(&str, i128)], owner: &str) -> World {
    World::new(
        bal.iter().map(|(a, b)| (a.to_string(), *b)).collect(),
        bal.iter().map(|(a, _)| (a.to_string(), owner.to_string())).collect(),
        0,
    )
}

fn s(x: &str) -> Value {
    Value::Str(x.into())
}

fn i(n: i128) -> Value {
    Value::Int(n)
}

// =====================================================================================
// Scale, size 1: branches (`if`/`else`), `ensure … when`, `emit … when` (docs/research/SCALE.md)
// =====================================================================================

fn escrow1() -> String {
    read("examples/escrow/escrow_s1.k")
}

const LEAF: &str = "
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
";

#[test]
fn s1_escrow_is_clean() {
    let (defs, reps) = analysed(&escrow1(), false);
    assert!(reps.values().all(|r| r.clean) && defs.defs.len() == 10);
}

#[test]
fn s1_only_the_else_path_is_wrong() {
    // The prover must check every path, not just the first: the bug is only in `else`.
    let src = format!(
        "{LEAF}
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
"
    );
    let (_, reps) = analysed(&src, false);
    let g = g_kind(&reps["pay"], "ensure", 0);
    assert!(refuted_and_replays(g));
    assert!(int(eg_call(g).1.last().unwrap()) > 0); // in the else path
}

#[test]
fn s1_wrong_when_condition_is_refuted() {
    let src = escrow1().replace(
        "ensure ledger.balance' seller = ledger.balance seller + amount when amount < 100",
        "ensure ledger.balance' seller = ledger.balance seller + amount when amount < 101",
    );
    let (_, reps) = analysed(&src, false);
    let g = g_kind(&reps["capture"], "ensure", 1);
    assert!(refuted_and_replays(g));
    assert_eq!(int(&eg_call(g).1[3]), 100); // exactly the boundary
}

#[test]
fn s1_wrong_emit_condition_is_refuted() {
    let src = escrow1().replace("emit Moved from to amount clock.now when amount > 0", "emit Moved from to amount clock.now when amount >= 0");
    let (_, reps) = analysed(&src, false);
    let g = g_kind(&reps["move"], "emit", 0);
    assert!(refuted_and_replays(g));
    assert_eq!(g.counterexample.as_ref().unwrap().emitted, "nothing"); // amount = 0
}

#[test]
fn s1_caller_emit_spec_composes_conditional_callee_events() {
    // tip's spec repeats move's conditional event; dropping the condition must be refuted.
    let src = escrow1().replace("emit Moved buyer seller amount clock.now when amount > 0", "emit Moved buyer seller amount clock.now");
    let (_, reps) = analysed(&src, false);
    assert!(refuted_and_replays(g_kind(&reps["tip"], "emit", 0)));
}

#[test]
fn s1_ok_otherwise_sees_a_need_inside_a_branch() {
    let src = escrow1().replace(
        "body\n  if amount > 0\n    transfer from to amount",
        "body\n  if amount > 0\n    need amount < 500 else TooBig\n    transfer from to amount",
    );
    let (_, reps) = analysed(&src, false);
    let g = ok_goal(&reps["move"]);
    assert!(refuted_and_replays(g));
    assert!(g.counterexample.as_ref().unwrap().eg.contains("TooBig"));
}

#[test]
fn s1_branch_need_is_lifted_as_an_implication() {
    // Callers may assume `amount <= 0 or amount < 500`, never `amount < 500` outright.
    let src = format!(
        "{LEAF}
small from:Account to:Account amount:Int
  ensure ledger.total' = ledger.total
body
  if amount > 0
    need amount < 500 else TooBig
    transfer from to amount
"
    );
    let defs = parse(&src);
    let conds: Vec<String> = lifted(&defs, &defs.defs["small"]).iter().filter(|l| l.ctor == "TooBig").map(|l| code(&l.cond)).collect();
    assert_eq!(conds, ["amount <= 0 or amount < 500"]);
}

#[test]
fn s1_writes_and_fails_cover_both_branches() {
    let defs = parse(&escrow1());
    let set = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(writes(&defs, &defs.defs["capture"]), set(&["escrow", "seller", "house"]));
    assert_eq!(writes(&defs, &defs.defs["refund"]), set(&["escrow", "buyer", "seller", "house"]));
    let fs = fails(&defs, &defs.defs["refund"]);
    for f in ["BadPart", "BadFee", "SameAccount", "Insufficient", "BadAmount"] {
        assert!(fs.contains_key(f), "{f}");
    }
}

#[test]
fn s1_refusal_inside_a_branch_rolls_back() {
    let src = format!("{LEAF}\ntwo a:Account b:Account amount:Int\nbody\n  transfer a b amount\n  if amount > 10\n    need amount < 20 else TooBig\n");
    let defs = parse(&src);
    let mut w = world(&[("A", 100), ("B", 0)], "u1");
    let out = Interp::new(&defs).run("two", &[s("A"), s("B"), i(50)], &mut w).unwrap();
    assert_eq!(out, Outcome::Fail("TooBig".into(), vec![]));
    assert_eq!(w.bal, world(&[("A", 100), ("B", 0)], "u1").bal);
    assert!(w.audit.is_empty());
}

#[test]
fn s1_straight_line_mutants_are_unchanged_and_branches_get_their_own() {
    let v5 = parse(&read("examples/ledger-tour/ledger_v5.k"));
    assert_eq!(mutants(&v5.defs["transfer"]).len(), 15);
    let descs: Vec<String> = mutants(&parse(&escrow1()).defs["capture"]).into_iter().map(|(d, _)| d).collect();
    for want in ["swap the branches of `if amount < 100`", "`<` → `<=` in `if amount < 100`", "drop `if amount < 100`", "drop `move escrow house fee`"] {
        assert!(descs.iter().any(|d| d == want), "{want}");
    }
}

#[test]
fn s1_page_shows_branch_refusals_and_conditions() {
    let (defs, reps) = analysed(&escrow1(), false);
    let mv = card_text(&reps["move"], &defs);
    assert!(mv.contains("amount ≠ 0 and from = to → SameAccount"), "{mv}");
    assert!(mv.contains("only when amount &gt; 0"));
    assert!(card_text(&reps["capture"], &defs).contains("when amount &lt; 100: balance of seller"));
}

#[test]
fn s1_one_infeasible_branch_is_not_vacuous() {
    let (_, reps) = analysed(
        "
f to:Account amount:Int
  need amount > 0 else Bad
  ensure ledger.balance' to = ledger.balance to + amount
body
  if amount < 0
    ledger.credit to 5
  else
    ledger.credit to amount
",
        false,
    );
    assert_eq!(statuses(&reps["f"]), ["proven"]);
}

// =====================================================================================
// Scale, size 2: declared state, invariants, `*` `/`, `need as` / `emit as` (docs/research/SCALE.md)
// =====================================================================================

fn escrow2() -> String {
    read("examples/escrow/escrow_s2.k")
}

const STATE_HEAD: &str = "state hold Int\nstate frozen Account\ninvariant a:Account ledger.balance a >= 0\n\n";

#[test]
fn s2_escrow_is_clean() {
    let (defs, reps) = analysed(&escrow2(), false);
    assert!(reps.values().all(|r| r.clean) && defs.defs.len() == 26);
}

#[test]
fn s2_state_dependent_refutation_replays_with_the_state_in_the_eg_line() {
    let src = escrow2().replacen(
        "  need spent.get a + amount <= limit.get a else OverLimit (spent.get a + amount - limit.get a)\n",
        "  need spent.get a + amount <= limit.get a + 1 else OverLimit (spent.get a + amount - limit.get a)\n",
        1,
    );
    let src = src.replacen(
        "  ok otherwise\n  keep ledger.total\n  ensure spent.get' a = spent.get a + amount\n",
        "  ok otherwise\n  keep ledger.total\n  ensure spent.get' a = spent.get a + amount\n  ensure spent.get' a <= limit.get a\n",
        1,
    );
    let (_, reps) = analysed(&src, false);
    let g = reps["withdraw"].guarantees.iter().find(|g| g.clause == "spent.get' a <= limit.get a").unwrap();
    assert!(refuted_and_replays(g));
    let eg = &g.counterexample.as_ref().unwrap().eg;
    assert!(eg.contains("spent.A:") || eg.contains("limit.A:"), "{eg}");
}

#[test]
fn s2_examples_check_state_after() {
    let src = escrow2().replace("| open_order 7 A E 100 -> ok | A:400 E:100 hold.7:100", "| open_order 7 A E 100 -> ok | A:400 E:100 hold.7:99");
    let (_, reps) = analysed(&src, false);
    assert!(!reps["open_order"].examples.iter().all(|e| e.pass));
}

#[test]
fn s2_state_writes_are_inferred_through_callees_and_frame_other_keys() {
    let src = format!(
        "{STATE_HEAD}
put id:Int n:Int
  ensure hold.get' id = n
body
  hold.set id n

two id:Int other:Int n:Int
  ensure hold.get' other = hold.get other when other != id
body
  put id n
"
    );
    let (defs, reps) = analysed(&src, false);
    assert_eq!(writes(&defs, &defs.defs["two"]).into_iter().collect::<Vec<_>>(), ["hold.id"]);
    assert_eq!(statuses(&reps["two"]), ["proven"]);
}

#[test]
fn s2_write_to_a_computed_key_is_any_key() {
    let defs = parse(&format!("{STATE_HEAD}f id:Int\nbody\n  hold.set (id + 1) 5\n"));
    assert_eq!(writes(&defs, &defs.defs["f"]).into_iter().collect::<Vec<_>>(), ["hold.*"]);
}

#[test]
fn s2_invariant_violation_is_refuted_and_replays() {
    let src = format!(
        "{STATE_HEAD}
drain a:Account b:Account amount:Int
  need a != b else SameAccount
  need amount > 0 else BadAmount
body
  ledger.debit a amount
  ledger.credit b amount
"
    );
    let (_, reps) = analysed(&src, false);
    assert!(refuted_and_replays(g_kind(&reps["drain"], "invariant", 0)));
}

#[test]
fn s2_invariant_with_a_computed_key_is_unknown_not_proven() {
    let (_, reps) = analysed("state hold Int\ninvariant o:Int hold.get o >= 0\n\nf id:Int\nbody\n  hold.set (id + 1) 5\n", false);
    let g = g_kind(&reps["f"], "invariant", 0); // never ✔; since S11 (f160617) ○, checked at run time by the host
    assert_eq!(g.status, Status::Runtime);
    assert!(g.reason.as_deref().unwrap_or("").contains("computed key"));
}

#[test]
fn s2_invariant_removes_the_overdrawn_workaround() {
    // purchase no longer needs `need ledger.balance escrow >= 0 else Overdrawn`.
    let src = escrow2();
    let (_, reps) = analysed(&src, false);
    assert!(!src.contains("Overdrawn") && ok_goal(&reps["purchase"]).status == Status::Proven);
}

#[test]
fn s2_division_agrees_between_prover_and_interpreter_for_negatives() {
    let (_, reps) = analysed(
        "
f to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to + amount / 2
  eg A:10@u1 | f A -7 -> ok | A:6
body
  ledger.credit to (amount / 2)
",
        false,
    );
    assert_eq!(statuses(&reps["f"]), ["proven"]);
    assert!(reps["f"].examples.iter().all(|e| e.pass));
}

#[test]
fn s2_need_as_expands_to_the_callees_spec_needs() {
    let defs = parse(&escrow2());
    let restated = parse(&escrow1());
    assert_eq!(defs.defs["reserve"].needs, restated.defs["reserve"].needs); // same conditions
    let (defs2, reps) = analysed(&escrow2(), false);
    // the author's source keeps the reference; a reader's spec view gets the conditions themselves
    assert!(views::editor(&reps, "reserve", &no_whys(&defs2)).contains("need as transfer buyer escrow amount"));
    let spec_view = views::spec(&reps["reserve"], &no_why());
    assert!(!spec_view.contains("need as") && spec_view.contains("need buyer != escrow else SameAccount"), "{spec_view}");
}

#[test]
fn s2_need_as_follows_the_spec_arguments_not_the_body() {
    // A body mutant that changes a callee argument is still caught: the reference names the arguments itself.
    let (_, reps) = analysed(&escrow2(), true);
    let m = reps["reserve"].mutation.as_ref().unwrap();
    assert_eq!((m.guarantee_killed, m.total), (6, 6));
}

#[test]
fn s2_emit_as_conjoins_its_condition() {
    let defs = parse(&escrow2());
    let conds: Vec<String> = defs.defs["refund"]
        .emits
        .iter()
        .filter_map(|e| match e {
            Emit::Ev(_, _, Some(c)) => Some(code(c)),
            _ => None,
        })
        .collect();
    assert!(!conds.iter().any(|c| c == "part = amount and amount > 0")); // release's events are unconditional
    assert!(conds.iter().any(|c| c == "part = amount") && conds.iter().any(|c| c == "part < amount and amount - part < 100"), "{conds:?}");
}

// =====================================================================================
// Scale, size 3: pure functions (-> Int) usable in specs, `distinct` (docs/research/SCALE.md)
// =====================================================================================

fn escrow3() -> String {
    read("examples/escrow/escrow_s3.k")
}

const FN: &str = "
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
";

#[test]
fn s3_escrow_is_clean() {
    let (defs, reps) = analysed(&escrow3(), false);
    assert!(reps.values().all(|r| r.clean) && defs.defs.len() == 55);
}

#[test]
fn s3_function_calls_are_modular() {
    // A caller's proof uses the function's spec only: a wrong body with the same spec leaves the caller proven
    // (and the function itself refuted, so trust propagation marks the caller).
    let (_, reps) = analysed(&FN.replace("    result = amount * 2 / 100\n", "    result = amount * 3 / 100\n"), false);
    assert!(statuses(&reps["fee_for"]).contains(&"refuted"));
    assert!(statuses(&reps["pay"]).iter().all(|s| *s == "conditional"));
}

#[test]
fn s3_same_arguments_same_result() {
    let src = format!(
        "{FN}
twice to:Account amount:Int
  ensure ledger.balance' to = ledger.balance to
body
  a = fee_for amount
  b = fee_for amount
  ledger.credit to (a - b)
"
    );
    let (_, reps) = analysed(&src, false);
    assert_eq!(statuses(&reps["twice"]), ["proven"]);
}

#[test]
fn s3_spec_named_functions_are_in_the_editor_view() {
    let (defs, reps) = analysed(&escrow3(), false);
    assert!(views::editor(&reps, "capture", &no_whys(&defs)).contains("fee_for amount:Int -> Int"));
}

#[test]
fn s3_distinct() {
    let (defs, reps) = analysed(FN, false);
    let text = card_text(&reps["pay"], &defs);
    assert!(text.contains("two of from , to and house are the same → SameAccount"), "{text}");
    let defs = parse("f a:Account n:Int\n  need distinct a n else X\nbody\n  ledger.credit a n\n");
    let err = kern::wf::check_module(&defs).err().map(|e| e.message()).unwrap_or_default();
    assert!(err.contains("distinct"), "{err}");
}

#[test]
fn s3_aliasing_bug_the_prover_found() {
    // Found while writing size 3: a cart that lets the buyer be the seller refuses when it tips.
    let src = escrow3().replace(
        "  need distinct buyer escrow seller house else SameAccount\n  need as purchase_order id buyer escrow seller house amount\n  need extra",
        "  need as purchase_order id buyer escrow seller house amount\n  need extra",
    );
    let (_, reps) = analysed(&src, false);
    assert!(refuted_and_replays(ok_goal(&reps["checkout_tip"])));
}

#[test]
fn s3_state_only_the_spec_reads_is_still_drawn_and_shown() {
    // Regression (size 3): `unfreeze` with `frozen.set a 0` dropped touches no state, but its spec reads
    // `frozen.get' a`; random worlds and counterexamples must still include `frozen`.
    let defs = parse(&escrow3());
    let mut md = defs.defs["unfreeze"].clone();
    md.body.retain(|s| !matches!(s, Stmt::Do(Expr::App(h, _, _)) if h == "frozen.set"));
    let mdefs = defs.with_def(md.clone());
    let (gs, _) = Prover::new(&mdefs).prove(&md, false);
    let g = gs.iter().find(|g| g.kind == "ensure").unwrap();
    assert!(g.status == Status::Refuted && g.counterexample.as_ref().unwrap().eg.contains("frozen.A:"), "{g:?}");
}

// =====================================================================================
// Roadmap run S4: bounded loops with invariants (docs/research/ROADMAP_RUN.md)
// =====================================================================================

fn batch() -> String {
    read("examples/batch/batch.k")
}

fn non_proven<'r>(reps: &'r IndexMap<String, Report>, name: &str) -> Vec<&'r Guarantee> {
    reps[name].guarantees.iter().filter(|g| g.status != Status::Proven).collect()
}

#[test]
fn s4_batch_is_clean_invariants_included() {
    let (_, reps) = analysed(&batch(), false);
    assert!(reps.values().all(|r| r.clean));
    assert!(reps["sweep_expired"].guarantees.iter().any(|g| g.kind == "loop" && g.clause.starts_with("module invariant")));
}

#[test]
fn s4_wrong_loop_invariants_are_refuted() {
    for (old, new, which) in [
        // an invariant that an iteration does not keep
        (
            "    invariant ledger.balance' buyer = ledger.balance buyer - 10 * k\n",
            "    invariant ledger.balance' buyer = ledger.balance buyer - 9 * k\n",
            "kept by every iteration",
        ),
        // an invariant false on entry
        (
            "    invariant ledger.balance' seller = ledger.balance seller + 10 * k\n",
            "    invariant ledger.balance' seller = ledger.balance seller + 10 * k + 1\n",
            "holds when the loop starts",
        ),
    ] {
        assert!(batch().contains(old));
        let (_, reps) = analysed(&batch().replace(old, new), false);
        let bad: Vec<_> = non_proven(&reps, "installments").into_iter().filter(|g| g.clause.contains(which)).collect();
        assert!(
            bad.iter().any(|g| matches!(g.status, Status::Refuted | Status::Unconfirmed)),
            "{which}: {:?}",
            bad.iter().map(|g| (&g.clause, g.status)).collect::<Vec<_>>()
        );
    }
}

#[test]
fn s4_off_by_one_bound_is_refuted_and_replays() {
    let (_, reps) = analysed(&batch().replace("  for k from 0 to n\n", "  for k from 0 to (n + 1)\n"), false);
    let bad = non_proven(&reps, "installments");
    assert!(!bad.is_empty() && bad.iter().any(|g| refuted_and_replays(g)));
}

#[test]
fn s4_quantified_ensure_false_at_one_key_is_refuted_with_the_key_in_the_eg_line() {
    let src = batch().replace(
        "    if sub_amount.get k > 0 and sub_due.get k <= clock.now\n",
        "    if sub_amount.get k > 0 and sub_due.get k < clock.now\n",
    );
    let (_, reps) = analysed(&src, false); // caught at the loop invariant (the ensure follows from it)
    let g: Vec<_> = reps["advance_subs"].guarantees.iter().filter(|g| g.status == Status::Refuted).collect();
    assert!(!g.is_empty());
    let c = g[0].counterexample.as_ref().unwrap();
    assert!(c.confirmed_by_interpreter && c.eg.contains("sub_due."), "{}", c.eg);
}

#[test]
fn s4_callers_use_the_loops_inferred_frame() {
    let (defs, reps) = analysed(&batch(), false);
    assert!(writes(&defs, &defs.defs["sweep_elsewhere"]).contains("hold.[lo,hi)"));
    assert!(reps["sweep_elsewhere"].guarantees.iter().all(|g| g.status == Status::Proven));
    let no_frame = batch().replace("  need other < lo or other >= hi else InRange\n", "");
    let (_, reps) = analysed(&no_frame, false);
    assert!(!non_proven(&reps, "sweep_elsewhere").is_empty()); // inside the range: not provable
}

#[test]
fn s4_events_inside_a_loop_are_flagged_never_proven() {
    let (defs, reps) = analysed(&batch(), false);
    assert_eq!(views::loop_event_note(&reps["sweep_expired"], &defs), ["audit"]);
    let out = render::terminal(&reps, &defs, &no_whys(&defs), &render::Orphans::new());
    assert!(out.contains("events emitted inside a loop are not described"));
    let src = batch().replace(
        "  keep ledger.total\n  ensure ledger.balance' buyer = ledger.balance buyer - 10 * n\n",
        "  keep ledger.total\n  ensure ledger.balance' buyer = ledger.balance buyer - 10 * n\n  emit Moved buyer seller 10 clock.now\n",
    );
    let (_, reps) = analysed(&src, false);
    let g = g_kind(&reps["installments"], "emit", 0);
    assert!(g.status != Status::Proven && g.reason.as_deref().unwrap_or("").contains("inside a loop"));
}

#[test]
fn s4_invariant_assumption_relies_on_every_writer() {
    // kern-90: the for-all-keys assumption is justified only if every writer of the store proves the invariant.
    // An unproven writer of `hold` downgrades the loop's proofs to `!`.
    let src = batch() + "\nspoil id:Int\nbody\n  hold.set id (0 - 1)\n";
    let (_, reps) = analysed(&src, false);
    assert!(reps["spoil"].guarantees.iter().any(|g| g.kind == "invariant" && g.status != Status::Proven));
    assert!(reps["sweep_expired"].guarantees.iter().all(|g| g.status == Status::Conditional));
    assert!(reps["sweep_expired"].guarantees[0].blocked_by.as_ref().unwrap().iter().any(|b| b == "invariant on hold"));
}

#[test]
fn s4_loop_mutants_exist() {
    let descs: Vec<String> = mutants(&parse(&batch()).defs["installments"]).into_iter().map(|(d, _)| d).collect();
    for want in ["`0` → `0 + 1` in `for k from 0 to n`", "`n` → `n - 1` in `for k from 0 to n`", "empty the body of `for k from 0 to n`"] {
        assert!(descs.iter().any(|d| d == want), "{want}");
    }
    assert!(mutants(&parse(&batch()).defs["sweep_expired"]).iter().any(|(d, _)| d.starts_with("`k` → `k + 1`")));
}

// ---- S4 fix-up: kern-90's wrong loop programs (tests/probes) carry the right tier ----------

fn probe(n: &str) -> (Module, Report) {
    let (defs, reps) = analysed(&read(&format!("tests/probes/{n}.k")), false);
    let mut rs: Vec<Report> = reps.into_iter().filter(|(name, _)| name != "transfer").map(|(_, r)| r).collect();
    assert_eq!(rs.len(), 1);
    (defs, rs.remove(0))
}

#[test]
fn probe_is_refuted_with_a_replaying_counterexample() {
    for (p, kind, part) in [
        ("p1", "ensure", None),
        ("p2", "ensure", None),
        ("p6", "ensure", None),
        ("p5", "ok", None),
        ("p3", "loop", Some("kept by every iteration")),
        ("p4", "loop", Some("module invariant")),
        ("p7", "loop", Some("holds when the loop starts")),
    ] {
        let (_, r) = probe(p);
        assert!(!r.clean, "{p}");
        let goals: Vec<_> = r.guarantees.iter().filter(|g| g.kind == kind && part.map_or(true, |x| g.clause.contains(x))).collect();
        assert!(goals.iter().any(|g| refuted_and_replays(g)), "{p}: {:?}", goals.iter().map(|g| (&g.clause, g.status)).collect::<Vec<_>>());
    }
}

#[test]
fn probe_p3_ensure_is_never_shown_proven_over_a_refuted_loop_invariant() {
    let (_, r) = probe("p3");
    let e = g_kind(&r, "ensure", 0);
    assert!(e.status == Status::Conditional && e.blocked_by.as_ref().unwrap().iter().any(|b| b.contains("loop invariant")));
}

#[test]
fn fz_vacuity_from_a_refuted_loop_invariant_is_not_never_succeeds() {
    // FZ finding 3: p7 said `Never succeeds` while its random tests succeeded 500/500.
    let (_, r) = probe("p7");
    assert!(!r.guarantees.iter().any(|g| g.status == Status::Vacuous));
    let (tone, text) = render::status(&r);
    assert!(tone == "fail" && text != "Never succeeds", "{tone} {text}");
    let e = g_kind(&r, "ensure", 0);
    assert!(e.status == Status::Conditional && e.blocked_by.as_ref().unwrap().iter().any(|b| b.contains("1 = 0")));
    assert!(e.vacuity.as_deref().unwrap_or("").contains("loop"));
}

#[test]
fn probe_p7_kept_goals_rest_on_the_refuted_invariant_and_vacuity_names_the_loop() {
    let (_, r) = probe("p7");
    let kept: Vec<_> = r.guarantees.iter().filter(|g| g.clause.contains("(kept by every iteration)")).collect();
    assert!(!kept.is_empty() && kept.iter().all(|g| g.status != Status::Proven));
    for g in r.guarantees.iter().filter(|g| g.status == Status::Vacuous || g.vacuity.is_some()) {
        let text = g.reason.as_deref().or(g.vacuity.as_deref()).unwrap_or("loop");
        assert!(text.contains("loop"), "{text}");
    }
}

// =====================================================================================
// Roadmap run S5: handlers declared in Kern with assumed specs (docs/research/ROADMAP_RUN.md)
// =====================================================================================

fn raffle() -> String {
    read("examples/raffle/raffle.k")
}

#[test]
fn s5_raffle_is_clean_and_assumed_guarantees_are_marked() {
    let (defs, reps) = analysed(&raffle(), false);
    assert!(reps.values().all(|r| r.clean));
    let st: std::collections::BTreeSet<&str> =
        reps["draw_winner"].guarantees.iter().filter(|g| g.kind != "pre").map(|g| g.status.as_str()).collect();
    assert_eq!(st.into_iter().collect::<Vec<_>>(), ["assumed"]);
    assert_eq!(render::status(&reps["draw_winner"]), ("ok", "Proven, assuming rand.below".to_string()));
    let out = render::terminal(&reps, &defs, &no_whys(&defs), &render::Orphans::new());
    assert!(out.starts_with("ASSUMES rand.below n:Int -> Int") && out.contains("⊙ ensure ledger.balance' pot"), "{out}");
    assert!(render::page(&reps, &defs, &no_whys(&defs), "raffle", &render::Orphans::new(), None).contains("<h2>Assumptions</h2>"));
}

#[test]
fn s5_a_weaker_assumption_cannot_prove_what_needs_the_stronger_one() {
    // pick_order's `picked < hi` needs `result < n`; draw_winner's pot guarantee doesn't (its else-branch pays for
    // any result >= 2).
    let (_, reps) = analysed(&raffle().replace("  ensure result >= 0 and result < n\n", "  ensure result >= 0\n"), false);
    let g = reps["pick_order"].guarantees.iter().find(|g| g.clause.starts_with("picked.get' 0")).unwrap();
    assert!(!matches!(g.status, Status::Proven | Status::Assumed));
    let pot = reps["draw_winner"].guarantees.iter().find(|g| g.clause.starts_with("ledger.balance' pot")).unwrap();
    assert_eq!(pot.status, Status::Assumed);
}

#[test]
fn s5_two_draws_may_differ() {
    let src = raffle() + "\ntwice to:Account\n  ensure ledger.balance' to = ledger.balance to\nbody\n  a = rand.below 5\n  b = rand.below 5\n  ledger.credit to (a - b)\n";
    let (_, reps) = analysed(&src, false);
    assert!(!matches!(reps["twice"].guarantees[0].status, Status::Proven | Status::Assumed));
}

#[test]
fn s5_a_provider_that_breaks_the_assumed_spec_is_a_fault() {
    let defs = parse(&raffle());
    kern::wf::check_module(&defs).unwrap();
    let mut w = world(&[("P", 100), ("A", 0), ("B", 0), ("C", 0)], "u");
    let provider: kern::run::Provider = Box::new(|_, _| Ok(Value::Int(7)));
    let r = Interp::with_provider(&defs, Some(provider), 0).run("draw_winner", &[s("P"), s("A"), s("B"), s("C"), i(30)], &mut w);
    assert!(matches!(&r, Err(RunError::Kern(m)) if m.contains("broke its assumed spec")), "{r:?}");
    assert_eq!(w.bal["P"], 100); // atomic
}

const NO_NEED: &str = "handler rand.below n:Int -> Int
  ensure result >= 0 and result < n

state hold Int

pick n:Int
  ensure hold.get' 0 >= 0
body
  r = rand.below n
  hold.set 0 r
";

#[test]
fn fz_a_provider_with_no_value_is_an_atomic_error_not_a_crash() {
    // FZ finding 2: no `need n > 0`, so rand.below 0 reaches randrange(0). Check must not crash, and the call
    // fails atomically with a KernError.
    let (defs, _) = analysed(NO_NEED, false); // used to raise ValueError
    let mut w = world(&[("A", 1)], "u");
    w.maps.insert("hold".into(), [(Key::Int(0), 4)].into_iter().collect());
    let r = Interp::new(&defs).run("pick", &[i(0)], &mut w);
    assert!(matches!(&r, Err(RunError::Kern(m)) if m.contains("gave no value")), "{r:?}");
    assert_eq!(w.maps["hold"], [(Key::Int(0), 4)].into_iter().collect::<IndexMap<_, _>>());
}

#[test]
fn fz_counterexample_search_has_a_time_limit() {
    // FZ finding 1: tests/probes/fz1.k hung in find_cex (no solver timeout after the first).
    let t = std::time::Instant::now();
    let (_, reps) = analysed(&read("tests/probes/fz1.k"), false);
    assert!(t.elapsed().as_secs() < 60);
    assert!(reps["f"].guarantees.iter().any(|g| g.status == Status::Refuted));
    assert!(!reps["f"].guarantees.iter().any(|g| g.kind == "ensure" && matches!(g.status, Status::Proven | Status::Assumed)));
}

#[test]
fn s5_handler_need_is_a_call_site_precondition_not_a_refusal() {
    let src = raffle().replace("  k = lo + rand.below (hi - lo)\n", "  k = lo + rand.below (hi - lo - 5)\n");
    let (defs, reps) = analysed(&src, false);
    let pre = g_kind(&reps["pick_order"], "pre", 0);
    assert!(refuted_and_replays(pre));
    assert!(!fails(&defs, &defs.defs["pick_order"]).contains_key("BadBound"));
}

#[test]
fn s5_assumed_specs_read_only_args_and_result() {
    for spec in ["  ensure result <= ledger.balance n\n", "  ensure result = hold.get n\n"] {
        let defs = parse(&format!("state hold Int\n\nhandler rand.below n:Int -> Int\n{spec}"));
        let err = kern::wf::check_module(&defs).err().map(|e| e.message()).unwrap_or_default();
        assert!(err.contains("only its arguments"), "{spec}: {err}");
    }
}

#[test]
fn s5_an_unsatisfiable_assumption_makes_callers_not_proven() {
    let (_, reps) = analysed(&raffle().replace("  ensure result >= 0 and result < n\n", "  ensure result >= n and result < n\n"), false);
    assert!(reps["draw_winner"].handler_sat["rand.below"].starts_with("unsatisfiable for n = "));
    assert!(reps["draw_winner"].guarantees.iter().filter(|g| g.kind != "pre").all(|g| !matches!(g.status, Status::Proven | Status::Assumed)));
}

#[test]
fn s5_the_ratchet_sees_a_changed_assumption() {
    // Python's _against_baseline: the baseline from a full check of the old source, then check the new one.
    let (bdefs, breps) = analysed(&raffle(), true);
    let base = kern::baseline::parse(&kern::baseline::text(&breps, &bdefs));
    let defs = parse(&raffle().replace("ensure result >= 0 and result < n", "ensure result >= 0 and result <= n"));
    let reps = analyse(&defs, true, Some(&base), None).unwrap();
    assert!(
        reps["draw_winner"].regression.iter().any(|x| x.starts_with("spec changed") && x.contains("assumes rand.below")),
        "{:?}",
        reps["draw_winner"].regression
    );
}

#[test]
fn s5_host_records_draws_and_replays_them() {
    let defs = parse(&raffle());
    kern::wf::check_module(&defs).unwrap();
    let reps = analyse(&defs, false, None, None).unwrap();
    let rng = std::rc::Rc::new(std::cell::RefCell::new(Random::from_int(3)));
    let r2 = rng.clone();
    let mut providers: IndexMap<String, kern::host::ProviderFn> = IndexMap::new();
    providers.insert(
        "rand.below".into(),
        Box::new(move |a: &[Value]| Ok(Value::Int(r2.borrow_mut().randrange(0, a[0].num().unwrap() as i64) as i128))),
    );
    let mut t = 0;
    let clock: Box<dyn FnMut() -> i128> = Box::new(move || {
        t += 1;
        t
    });
    let mut h = Host::new(&defs, reps.clone(), Options { clock: Some(clock), sinks: Some(vec![]), providers: Some(providers), ..Default::default() }).unwrap();
    h.init("P:100000@u0 A:0@u1 B:0@u2 C:0@u3").unwrap();
    for k in 0..200 {
        h.call("draw_winner", &[s("P"), s("A"), s("B"), s("C"), i(1 + k % 7)]).unwrap();
        h.call("pick_order", &[i(k), i(k + 1 + k % 4)]).unwrap();
    }
    assert!(h.replay().unwrap().is_empty());
    let obs: Vec<String> = h.db.prepare("select observed from log").unwrap().query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    assert!(obs.iter().all(|o| {
        let v: serde_json::Value = serde_json::from_str(o).unwrap();
        v.as_array().is_some_and(|a| a.len() == 1 && a[0][0] == "rand.below")
    }));
    let missing = Host::new(&defs, reps, Options { sinks: Some(vec![]), providers: Some(IndexMap::new()), ..Default::default() });
    assert!(matches!(&missing, Err(e) if e.0.contains("does not provide")));
}

#[test]
fn probe_tiers_record() {
    // With KERN_PROBE_DUMP=path, record every probe goal's tier (the S4 fix-up order check compares these files).
    let mut tiers = serde_json::Map::new();
    for p in ["p1", "p2", "p3", "p4", "p5", "p6", "p7"] {
        let (_, r) = probe(p);
        let gs: Vec<serde_json::Value> = r.guarantees.iter().map(|g| serde_json::json!([g.kind, g.clause, g.status.as_str()])).collect();
        tiers.insert(p.into(), gs.into());
    }
    if let Ok(path) = std::env::var("KERN_PROBE_DUMP") {
        std::fs::write(path, serde_json::to_string_pretty(&tiers).unwrap()).unwrap();
    }
    assert!(!tiers.is_empty());
}
