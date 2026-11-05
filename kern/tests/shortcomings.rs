//! Gap tests for the code-gap entries of docs/research/SHORTCOMINGS.md (S5–S12), so the register can't go stale
//! silently.
//!
//! Each `#[should_panic(expected = "gap Sn …")]` test asserts the behaviour we WANT, so today it panics with its
//! own message. An unrelated crash or a typo panics with a different message, so it doesn't count as the gap.
//! The day a gap closes, its test fails. In the same commit it becomes an ordinary test (drop `should_panic`),
//! and the entry moves to "Closed" in SHORTCOMINGS.md.
//!
//! Some gaps have no syntax yet (records, text, `match`, named constants, `sum`, `clock.tick`, events in `eg`).
//! Those tests write a *placeholder* syntax, marked `placeholder:`. Whoever builds the feature adjusts the
//! source to the syntax actually chosen, and the test must then pass. A test that still panics after a feature
//! has landed means the placeholder was never updated, not that the gap is still open.
//!
//! The last test checks the register against this crate: every `kern/tests/…` path and every test name it
//! mentions must exist.

mod common;
use common::{tmp, write_tree, ROOT};

use indexmap::IndexMap;
use kern::check::{mutants, mutation_score};
use kern::host::{load_config, CallResult, Endpoint, Host, Net, Options};
use kern::report::{analyse, Report};
use kern::run::Value;
use kern::syntax::{parse_module, Key, Module};
use kern::views;
use kern::why;

fn read(p: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/{p}")).unwrap()
}

fn parse(src: &str) -> Module {
    parse_module(src, true).unwrap()
}

fn reports(defs: &Module) -> IndexMap<String, Report> {
    analyse(defs, false, None, None).unwrap()
}

/// Panics with `"{gap}: …"` unless `src` parses, is well-formed and every definition is clean. Without mutation
/// testing, to stay fast.
fn must_check(gap: &str, src: &str) {
    let defs = parse_module(src, true).unwrap_or_else(|e| panic!("{gap}: doesn't parse: {}", e.0));
    let reps = analyse(&defs, false, None, None).unwrap_or_else(|e| panic!("{gap}: not well-formed: {}", e.message()));
    let dirty: Vec<&String> = reps.iter().filter(|(_, r)| !r.clean).map(|(n, _)| n).collect();
    assert!(dirty.is_empty(), "{gap}: not clean: {dirty:?}");
}

// =====================================================================================
// S5. A small language: one test per missing feature, a source that should check and doesn't
// =====================================================================================

#[test]
#[should_panic(expected = "gap S5 records:")]
fn s5_records_check() {
    // gap (S5): no records; parameters are Int and Account only. placeholder: `record` declaration, field access
    must_check(
        "gap S5 records",
        "record Point x:Int y:Int

state px Int

move_x k:Int p:Point dx:Int
  ok otherwise
  ensure px.get' k = p.x + dx
body
  px.set k (p.x + dx)
",
    );
}

#[test]
#[should_panic(expected = "gap S5 text:")]
fn s5_text_checks() {
    // gap (S5): no text. examples/random-portal stores joke ids, not jokes. placeholder: `Text`, `length`
    must_check(
        "gap S5 text",
        "state title Text

name_it slot:Int t:Text
  need length t > 0 else Empty
  ok otherwise
  ensure title.get' slot = t
body
  title.set slot t
",
    );
}

#[test]
#[should_panic(expected = "gap S5 match:")]
fn s5_match_checks() {
    // gap (S5): no `match`; branches are nested `if`/`else`. placeholder: `match E` with `V -> stmt` arms
    must_check(
        "gap S5 match",
        "state mode Int

pick src:Int
  ok otherwise
  ensure mode.get' src = 10 when src = 1
  ensure mode.get' src = 20 when src = 2
  ensure mode.get' src = 0 when src != 1 and src != 2
body
  match src
    1 -> mode.set src 10
    2 -> mode.set src 20
    _ -> mode.set src 0
",
    );
}

#[test]
#[should_panic(expected = "gap S5 recursion:")]
fn s5_recursion_is_accepted() {
    // gap (S5): recursion is rejected (`recursive (down -> down); no termination measure yet`). Real syntax today;
    // a termination measure, when it exists, may need a line of its own
    must_check(
        "gap S5 recursion",
        "state cnt Int

down n:Int
  need n >= 0 else Negative
  ok otherwise
  ensure cnt.get' 0 = cnt.get 0 + n
body
  if n > 0
    cnt.set 0 (cnt.get 0 + 1)
    down (n - 1)
",
    );
}

#[test]
#[should_panic(expected = "gap S5 nested loops:")]
fn s5_nested_loops_check() {
    // gap (S5): `nested loops are not supported yet`. Real syntax today
    must_check(
        "gap S5 nested loops",
        "state cell Int

invariant x:Int cell.get x >= 0

clear_twice n:Int
  ok otherwise
  ensure cell.get' j >= 0 for j from 0 to n
body
  for i from 0 to 2
    invariant cell.get' j >= 0 for j from 0 to n
    for k from 0 to n
      invariant cell.get' j >= 0 for j from 0 to n
      cell.set k 0
",
    );
}

#[test]
#[should_panic(expected = "gap S5 variable times variable:")]
fn s5_variable_times_variable_checks() {
    // gap (S5): arithmetic is linear (`*` needs a number on one side). Real syntax today
    must_check(
        "gap S5 variable times variable",
        "state area Int

measure k:Int w:Int h:Int
  ok otherwise
  ensure area.get' k = w * h
body
  area.set k (w * h)
",
    );
}

#[test]
#[should_panic(expected = "gap S5 named constants:")]
fn s5_named_constants_check() {
    // gap (S5): no named constants. placeholder: `const NAME VALUE`
    must_check(
        "gap S5 named constants",
        "const Fee 5

state paid Int

charge k:Int
  ok otherwise
  ensure paid.get' k = paid.get k + Fee
body
  paid.set k (paid.get k + Fee)
",
    );
}

#[test]
#[should_panic(expected = "gap S5 cross-package import:")]
fn s5_import_across_packages_loads() {
    // gap (S5): imports reach within one package only, so each example app copies its ledger. Two sibling
    // packages, `app` imports a module of `money`. placeholder: how `app` names its dependency (a `kern.pkg` line)
    let gap = "gap S5 cross-package import";
    let root = tmp("s5-cross-package");
    write_tree(&root.join("money"), &[("ledger.k", &read("examples/shop/money/ledger.k"))], true);
    let app = write_tree(
        &root.join("app"),
        &[
            ("kern.pkg", "use ../money\n"),
            (
                "pay.k",
                "import money.ledger

pay a:Account b:Account
  need as transfer a b 5
  ok otherwise
  keep ledger.total
  emit as transfer a b 5
body
  transfer a b 5
",
            ),
        ],
        false,
    );
    let defs = kern::package::load(&app.join("pay.k").to_string_lossy(), &IndexMap::new()).unwrap_or_else(|e| panic!("{gap}: doesn't load: {}", e.0));
    let reps = analyse(&defs, false, None, None).unwrap_or_else(|e| panic!("{gap}: not well-formed: {}", e.message()));
    assert!(reps.values().all(|r| r.clean), "{gap}: not clean");
}

// =====================================================================================
// S6. Loops: the frame can't be stated, and there is no sum
// =====================================================================================

#[test]
#[should_panic(expected = "gap S6 loop frame:")]
fn s6_a_guarantee_kills_the_sweep_one_too_many_mutant() {
    // gap (S6): batch.k's `hi + 1` and `lo - 1` loop-bound mutants survive every guarantee, because "every key
    // outside [lo, hi) is unchanged" can't be stated (the frame is inferred). Closing it means adding the stated
    // frame to batch.k's advance_subs; then no bound mutant survives the guarantees.
    let defs = parse(&read("examples/batch/batch.k"));
    let m = mutation_score(&defs, &defs.defs["advance_subs"], 150);
    let bound: Vec<&String> = m.guarantee_missed.iter().map(|(d, _)| d).filter(|d| d.contains("hi + 1") || d.contains("lo - 1")).collect();
    assert!(bound.is_empty(), "gap S6 loop frame: no guarantee catches {bound:?}");
}

#[test]
#[should_panic(expected = "gap S6 sum:")]
fn s6_a_sum_invariant_proves() {
    // gap (S6): no sum; conservation is pairwise only, so "the pool gains the sum of the swept holds" can't be
    // said. placeholder: `sum j from A to B of E`
    must_check(
        "gap S6 sum",
        "state hold Int

invariant o:Int hold.get o >= 0

sweep pool:Account lo:Int hi:Int
  ok otherwise
  ensure ledger.balance' pool = ledger.balance pool + sum j from lo to hi of hold.get j
  ensure hold.get' j = 0 for j from lo to hi
body
  for k from lo to hi
    invariant ledger.balance' pool = ledger.balance pool + sum j from lo to k of hold.get j
    invariant hold.get' j = 0 for j from lo to k
    ledger.credit pool (hold.get k)
    hold.set k 0
",
    );
}

// =====================================================================================
// S7. Refusal conditions after a write (closed: lifted through direct writes, with aliasing)
// =====================================================================================

const S7_SPEC: &str = "pay from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  keep ledger.total
body
";

fn s7_spec_view(body: &str) -> String {
    let defs = parse(&format!("{S7_SPEC}{body}"));
    views::spec(&reports(&defs)["pay"], &why::Status::default())
}

/// The lifted conditions of `name` in `src`, as code (cond, ctor, payload given).
fn s7_lifted(src: &str, name: &str) -> Vec<(String, String, bool)> {
    let defs = parse(src);
    kern::infer::lifted(&defs, &defs.defs[name]).iter().map(|l| (kern::syntax::code(&l.cond), l.ctor.clone(), l.payload.is_some())).collect()
}

#[test]
fn s7_a_refusal_after_a_write_shows_in_the_spec_view() {
    // was a gap (S7): a body `need` after the first ledger write wasn't lifted. Before the write it always showed.
    let before = s7_spec_view("  need ledger.balance to < 1000 else TooRich\n  ledger.debit from amount\n  ledger.credit to amount\n");
    assert!(before.contains("body need ledger.balance to < 1000 else TooRich"), "control: {before}");
    // after `ledger.debit from amount`: `to` is not `from` (a spec need says so), so its balance is unchanged
    let after = s7_spec_view("  ledger.debit from amount\n  need ledger.balance to < 1000 else TooRich\n  ledger.credit to amount\n");
    assert!(after.contains("body need ledger.balance to < 1000 else TooRich"), "{after}");
}

#[test]
fn s7_a_condition_after_a_write_is_rewritten_into_the_pre_state() {
    let body = |b: &str| format!("pay from:Account to:Account amount:Int\nbody\n{b}");
    // the written account itself: no case split
    assert_eq!(
        s7_lifted(&body("  ledger.debit from amount\n  need ledger.balance from >= 0 else Overdrawn (ledger.balance from)\n"), "pay"),
        [("ledger.balance from - amount >= 0".to_string(), "Overdrawn".to_string(), true)]
    );
    // another account that may be the same one: one case each way, and no payload (it differs between them)
    assert_eq!(
        s7_lifted(&body("  ledger.debit from amount\n  need ledger.balance to < 1000 else TooRich (ledger.balance to)\n"), "pay"),
        [("to = from and ledger.balance to - amount < 1000 or to != from and ledger.balance to < 1000".to_string(), "TooRich".to_string(), false)]
    );
    // a `need from != to` passed earlier in the body prunes the aliasing case, as a spec need does
    assert_eq!(
        s7_lifted(&body("  need from != to else Same\n  ledger.debit from amount\n  need ledger.balance to < 1000 else TooRich\n"), "pay")[1],
        ("ledger.balance to < 1000".to_string(), "TooRich".to_string(), true)
    );
    // the total moves by every write; state maps split on the key
    assert_eq!(
        s7_lifted(&body("  ledger.credit to amount\n  need ledger.total <= 500 else Full\n"), "pay"),
        [("ledger.total + amount <= 500".to_string(), "Full".to_string(), true)]
    );
    let src = "state hold Int\n\nput k:Int j:Int v:Int\nbody\n  hold.set k v\n  need hold.get j > 0 else Empty\n";
    assert_eq!(s7_lifted(src, "put"), [("j = k and v > 0 or j != k and hold.get j > 0".to_string(), "Empty".to_string(), true)]);
    // the printed condition parses back to itself (`and` binds tighter than `or`)
    let defs = parse(&body("  ledger.debit from amount\n  need ledger.balance to < 1000 else TooRich\n"));
    let l = &kern::infer::lifted(&defs, &defs.defs["pay"])[0];
    assert_eq!(kern::syntax::parse_expr(&kern::syntax::code(&l.cond), None).unwrap(), l.cond);
}

#[test]
fn s7_lifting_still_stops_where_the_after_state_is_not_exact() {
    // after a callee that writes (its after-state is only its spec), after a loop with effects, and after an `if`
    // whose branches write, a state-reading condition is not lifted; one that reads no state still is
    let src = "state hold Int

move a:Account b:Account
  keep ledger.total
body
  ledger.debit a 1
  ledger.credit b 1

via_callee a:Account b:Account n:Int
body
  move a b
  need ledger.balance a >= 0 else Low
  need n > 0 else Bad

via_loop a:Account n:Int
body
  for k from 0 to n
    hold.set k 0
  need hold.get 0 >= 0 else Low

via_if a:Account n:Int
body
  if n > 0
    ledger.debit a 1
  need ledger.balance a >= 0 else Low

inside_if a:Account n:Int
body
  if n > 0
    ledger.debit a 1
    need ledger.balance a >= 0 else Low
";
    let ctors = |n: &str| s7_lifted(src, n).into_iter().map(|(c, k, _)| format!("{c} else {k}")).collect::<Vec<_>>();
    assert_eq!(ctors("via_callee"), ["n > 0 else Bad"]);
    assert!(ctors("via_loop").is_empty());
    assert!(ctors("via_if").is_empty());
    // inside the branch that wrote, the write is exact: lifted under the branch's guard
    assert_eq!(ctors("inside_if"), ["n <= 0 or ledger.balance a - 1 >= 0 else Low"]);
}

// =====================================================================================
// S17. Lifting past a writing callee whose spec pins what it writes
// =====================================================================================

const S17_MOVE: &str = "move a:Account b:Account n:Int
  need a != b else Same
  keep ledger.total
  ensure ledger.balance' a = ledger.balance a - n
  ensure ledger.balance' b = ledger.balance b + n
body
  ledger.debit a n
  ledger.credit b n
";

const S17_REST: &str = "
step a:Account b:Account n:Int
body
  move a b n
  need ledger.balance a <= 10 else Big
  need ledger.total >= 0 else Neg

outer x:Account y:Account m:Int
  ensure ledger.balance x - m <= 10
body
  step x y m
";

fn s17_status(src: &str, def: &str) -> (&'static str, String) {
    let reps = reports(&parse(src));
    let g = reps[def].guarantees.iter().find(|g| g.kind == "ensure").unwrap();
    (g.status.as_str(), format!("{:?}", g.blocked_by))
}

#[test]
fn s17_a_need_after_a_pinned_call_is_lifted_and_proves_the_callers_claim() {
    let src = format!("{S17_MOVE}{S17_REST}");
    let defs = parse(&src);
    let l: Vec<(String, Vec<String>)> =
        kern::infer::lifted(&defs, &defs.defs["step"]).iter().map(|l| (kern::syntax::code(&l.cond), l.rests_on.clone())).collect();
    // move's spec pins a and b (and keeps the total); its `a != b` prunes the aliasing case
    assert!(l.contains(&("ledger.balance a - n <= 10".to_string(), vec!["move".to_string()])), "{l:?}");
    assert!(l.contains(&("ledger.total >= 0".to_string(), vec!["move".to_string()])), "{l:?}");
    assert_eq!(s17_status(&src, "outer").0, "proven");
    let view = views::spec(&reports(&defs)["step"], &why::Status::default());
    assert!(view.contains("body need ledger.balance a - n <= 10 else Big"), "{view}");
}

#[test]
fn s17_a_claim_resting_on_a_wrong_pinned_spec_is_at_most_conditional_and_hidden_from_views() {
    // move pins a wrong value for a. step has no guarantees, so it is clean; outer's claim rests on move's spec
    // through step's lifted need, and must come out `!` blocked by move, never ✔. The claim is in fact false:
    let wrong = S17_MOVE.replace("ledger.balance a - n\n", "ledger.balance a - n + 1\n");
    let src = format!("{wrong}{S17_REST}").replace("ensure ledger.balance x - m <= 10", "ensure ledger.balance x - m + 1 <= 10");
    let (status, blocked) = s17_status(&src, "outer");
    assert_eq!(status, "conditional");
    assert!(blocked.contains("move"), "{blocked}");
    let defs = parse(&src);
    let mut w = kern::run::World::new(
        [("X".to_string(), 10), ("Y".to_string(), 0)].into_iter().collect(),
        [("X".to_string(), "u1".to_string()), ("Y".to_string(), "u1".to_string())].into_iter().collect(),
        0,
    );
    let args = [Value::Str("X".into()), Value::Str("Y".into()), Value::Int(0)];
    // outer X Y 0 with X at 10 succeeds, and the claim says 10 - 0 + 1 <= 10
    assert_eq!(kern::run::Interp::new(&defs).run("outer", &args, &mut w).unwrap(), kern::run::Outcome::Ok);
    // (b): the line is not shown, since what it rests on isn't clean
    let reps = reports(&defs);
    assert!(!reps["move"].clean);
    let view = views::spec(&reps["step"], &why::Status::default());
    assert!(!view.contains("body need"), "{view}");
    // the prover still knows the condition exists: it is in the inferred interface (anchors), only not displayed
    assert!(kern::infer::lifted(&defs, &defs.defs["step"]).iter().any(|l| l.ctor == "Big"));
}

#[test]
fn s17_lifting_stops_after_a_callee_that_does_not_pin_what_it_writes() {
    let loose = S17_MOVE.replace("ensure ledger.balance' b = ledger.balance b + n", "ensure ledger.balance' b >= ledger.balance b");
    let defs = parse(&format!("{loose}{S17_REST}"));
    let ctors: Vec<String> = kern::infer::lifted(&defs, &defs.defs["step"]).iter().map(|l| l.ctor.clone()).collect();
    assert_eq!(ctors, ["Same"], "only move's own need is lifted: b's new balance isn't pinned");
    // pinned balances without `keep ledger.total`: balances lift, the total doesn't
    let unkept = S17_MOVE.replace("  keep ledger.total\n", "");
    let defs = parse(&format!("{unkept}{S17_REST}"));
    let ctors: Vec<String> = kern::infer::lifted(&defs, &defs.defs["step"]).iter().map(|l| l.ctor.clone()).collect();
    assert_eq!(ctors, ["Same", "Big"]);
}

// =====================================================================================
// S8. Mutants are body-only
// =====================================================================================

#[test]
#[should_panic(expected = "gap S8 spec mutants:")]
fn s8_spec_need_mutants_are_generated() {
    // gap (S8): moving a `need` from the body into the spec removes its mutants. Control first: in the body,
    // `k > 0` is mutated.
    let body_need = parse("state cnt Int\n\nbump k:Int\n  ensure cnt.get' k = cnt.get k + 1\nbody\n  need k > 0 else BadKey\n  cnt.set k (cnt.get k + 1)\n");
    let spec_need = parse("state cnt Int\n\nbump k:Int\n  need k > 0 else BadKey\n  ok otherwise\n  ensure cnt.get' k = cnt.get k + 1\nbody\n  cnt.set k (cnt.get k + 1)\n");
    let on_need = |defs: &Module| mutants(&defs.defs["bump"]).into_iter().filter(|(d, _)| d.contains("k > 0")).count();
    assert!(on_need(&body_need) > 0, "control: no body mutant of `k > 0`");
    let n = on_need(&spec_need);
    assert!(n > 0, "gap S8 spec mutants: no mutant of the spec's `need k > 0`");
}

// =====================================================================================
// S10. Audit events and time are coarse
// =====================================================================================

#[test]
fn s10_emits_nothing_is_expressible_today_as_a_false_conditional_emit() {
    // not a gap (a finding, 2026-11-05): `emit Ctor when <false>` is an exact empty sequence. A body that emits
    // is refuted with the event in the counterexample; a body that emits nothing proves. What's missing is a
    // readable form (the page says "emit Never when 1 = 0"), which is ergonomics, not expressiveness.
    let spec = "quiet a:Account\n  keep ledger.total\n  emit Never when 1 = 0\nbody\n";
    let emit_status = |body: &str| {
        let defs = parse(&format!("{spec}{body}"));
        let r = &reports(&defs)["quiet"];
        r.guarantees.iter().find(|g| g.kind == "emit").unwrap().status.as_str()
    };
    assert_eq!(emit_status("  audit.emit (Ping a clock.now)\n"), "refuted");
    assert_eq!(emit_status("  ledger.debit a 0\n"), "proven");
}

#[test]
#[should_panic(expected = "gap S10 eg events:")]
fn s10_eg_can_assert_events() {
    // gap (S10): an `eg` line can't say which events the call emits. An example expecting the wrong event must
    // fail. placeholder: a fourth `|` section listing the events
    let gap = "gap S10 eg events";
    let src = "transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  emit Moved from to amount clock.now
  eg A:500@u1 B:0@u2 | transfer A B 100 -> ok | A:400 B:100 | Moved B A 100 0
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (Moved from to amount clock.now)
";
    let defs = parse_module(src, true).unwrap_or_else(|e| panic!("{gap}: doesn't parse: {}", e.0));
    let ex = kern::check::run_examples(&defs, &defs.defs["transfer"]).unwrap();
    assert!(ex.iter().any(|e| !e.pass), "{gap}: an example expecting the wrong event passes");
}

#[test]
#[should_panic(expected = "gap S10 instants:")]
fn s10_one_call_can_see_two_instants() {
    // gap (S10): `clock.now` is one value per top-level call. placeholder: `clock.tick`
    must_check(
        "gap S10 instants",
        "state seen Int

stamp k:Int
  ok otherwise
  ensure seen.get' k > clock.now
body
  clock.tick
  seen.set k clock.now
",
    );
}

// =====================================================================================
// S12. The host: a request with no reply stays pending forever
// =====================================================================================

/// A network that never answers: no reply call and no `fail` call, ever.
struct Silent;
impl Net for Silent {
    fn send(&mut self, _: i64, _: &Value, _: &IndexMap<String, Endpoint>) -> Vec<(String, Vec<Value>)> {
        vec![]
    }
}

#[test]
#[should_panic(expected = "gap S12 request timeout:")]
fn s12_a_request_with_no_reply_times_out_into_a_call() {
    // gap (S12): examples/limits asks for A's limit; the network never answers. An hour per call later, the
    // request should have timed out into a new call (`limit_failed`, as a failed reply would), freeing
    // `pending`. Today it stays pending. (HttpNet's 2 s transport timeout does become `fail`; this is about a
    // reply that never reaches the host at all.)
    let defs = parse(&read("examples/limits/limits.k"));
    kern::wf::check_module(&defs).unwrap();
    let eps = load_config(&read("examples/limits/limits.host"), &defs).unwrap();
    let mut t = 0;
    let clock = Box::new(move || {
        t += 3600;
        t
    });
    let o = Options { clock: Some(clock), sinks: Some(vec![]), endpoints: Some(eps), net: Some(Box::new(Silent)), ..Default::default() };
    let mut h = Host::new(&defs, reports(&defs), o).unwrap();
    h.init("A:500@u1 B:500@u2 K:0@u9").unwrap();
    let call = |h: &mut Host, name: &str, a: &str| -> CallResult { h.call(name, &[Value::Str(a.into())]).unwrap() };
    assert_eq!(call(&mut h, "request_limit", "A")["status"], "ok", "control");
    let mut ran = vec![];
    for _ in 0..24 {
        call(&mut h, "reset_day", "B");
        h.flush().unwrap();
        ran.extend(h.pump(vec![]).unwrap().into_iter().map(|(n, _, _)| n));
    }
    let pending = h.load().unwrap().maps["pending"].get(&Key::Str("A".into())).copied();
    assert!(pending == Some(0), "gap S12 request timeout: after 24 hours pending.A = {pending:?}, reply calls run: {ran:?}");
}

// =====================================================================================
// The register itself: what it names must exist
// =====================================================================================

#[test]
fn every_test_and_path_the_register_names_exists() {
    let text = read("docs/research/SHORTCOMINGS.md");
    let paths = regex::Regex::new(r"kern/tests/[\w/]+\.rs(?::(\d+))?").unwrap();
    let mut n_paths = 0;
    for c in paths.captures_iter(&text) {
        let p = c[0].split(':').next().unwrap();
        let src = std::fs::read_to_string(format!("{ROOT}/{p}")).unwrap_or_else(|_| panic!("SHORTCOMINGS.md names `{p}`, which doesn't exist"));
        if let Some(line) = c.get(1) {
            let line: usize = line.as_str().parse().unwrap();
            let near: Vec<&str> = src.lines().skip(line.saturating_sub(3)).take(5).collect();
            assert!(near.iter().any(|l| l.contains("fn ")), "SHORTCOMINGS.md points at `{}`, which is not at a test any more", &c[0]);
        }
        n_paths += 1;
    }
    // test names: backticked identifiers named after a fundamental or an entry (f5_…, s7_…)
    let names = regex::Regex::new(r"`([fs]\d+_[a-z0-9_]+)`").unwrap();
    let mut all = String::new();
    for dir in ["kern/tests", "validation/tests"] {
        for e in std::fs::read_dir(format!("{ROOT}/{dir}")).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "rs") {
                all.push_str(&std::fs::read_to_string(e.path()).unwrap());
            }
        }
    }
    let mut n_names = 0;
    for c in names.captures_iter(&text) {
        assert!(all.contains(&format!("fn {}(", &c[1])), "SHORTCOMINGS.md names test `{}`, which doesn't exist", &c[1]);
        n_names += 1;
    }
    assert!(n_paths >= 1 && n_names >= 1, "found {n_paths} paths and {n_names} test names: the patterns no longer match the register");
}
