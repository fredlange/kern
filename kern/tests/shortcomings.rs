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
// S7. Refusal conditions are lifted only up to the first ledger write
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

#[test]
#[should_panic(expected = "gap S7 lifted refusal:")]
fn s7_a_refusal_after_a_write_shows_in_the_spec_view() {
    // gap (S7): a body `need` after the first ledger write isn't lifted, so neither the definition's spec view
    // nor a caller shows when it refuses (only `fails … TooRich`). Control first: before the write it shows.
    let before = s7_spec_view("  need ledger.balance to < 1000 else TooRich\n  ledger.debit from amount\n  ledger.credit to amount\n");
    assert!(before.contains("body need ledger.balance to < 1000 else TooRich"), "control: {before}");
    let after = s7_spec_view("  ledger.debit from amount\n  need ledger.balance to < 1000 else TooRich\n  ledger.credit to amount\n");
    assert!(after.contains("ledger.balance to < 1000 else TooRich"), "gap S7 lifted refusal: not in the spec view:\n{after}");
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
