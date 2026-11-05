//! Gap tests for the code-gap entries of docs/research/SHORTCOMINGS.md (S5–S12), so the register can't go stale
//! silently.
//!
//! Each `#[should_panic(expected = "gap Sn …")]` test asserts the behaviour we WANT, so today it panics with its
//! own message. An unrelated crash or a typo panics with a different message, so it doesn't count as the gap.
//! The day a gap closes, its test fails. In the same commit it becomes an ordinary test (drop `should_panic`),
//! and the entry moves to "Closed" in SHORTCOMINGS.md.
//!
//! Some gaps have no syntax yet (text, `match`, `sum`, `clock.tick`, events in `eg`).
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
fn s5_records_check() {
    // closed (S5): record-typed parameters, flattened to their fields at load time (DECISIONS.md, 2026-11-05)
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

const RECORD_SRC: &str = "record Point x:Int y:Int

state px Int
state py Int

move_x k:Int p:Point dx:Int
  ok otherwise
  ensure px.get' k = p.x + dx
  eg | move_x 1 3 4 2 -> ok | px.1:5
body
  px.set k (p.x + dx)

place k:Int p:Point
  ok otherwise
  ensure px.get' k = p.x
  ensure py.get' k = p.y
body
  move_x k p 0
  py.set k p.y

origin k:Int
  ok otherwise
  ensure py.get' k = 7
body
  place k (Point 0 7)

norm p:Point -> Int
  ensure result = p.x + p.y
body
  result = p.x + p.y
";

#[test]
fn s5_a_record_parameter_is_its_fields_to_the_checker_and_declared_in_views() {
    let defs = parse(RECORD_SRC);
    let params: Vec<(&str, &str)> = defs.defs["move_x"].params.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    assert_eq!(params, [("k", "Int"), ("p.x", "Int"), ("p.y", "Int"), ("dx", "Int")]);
    // passing a record whole, and building one as an argument, become its fields in order
    assert!(defs.defs["place"].body.iter().any(|s| kern::syntax::stmt_code(s) == "move_x k p.x p.y 0"));
    assert!(defs.defs["origin"].body.iter().any(|s| kern::syntax::stmt_code(s) == "place k 0 7"));
    let reps = reports(&defs);
    let dirty: Vec<&String> = reps.iter().filter(|(_, r)| !r.clean).map(|(n, _)| n).collect();
    assert!(dirty.is_empty(), "{dirty:?}");
    // the header stays as written, and a caller reads the fields it needs to build one
    let view = views::iface(&reps["place"], &why::Status::default());
    assert!(view.starts_with("place k:Int p:Point\n  record Point x:Int y:Int\n  uses"), "{view}");
}

#[test]
fn s5_a_counterexample_for_a_record_parameter_replays_as_an_eg_line() {
    // eg arguments are positional (p is `3 4`), and so are counterexamples: F8's round trip holds
    let src = RECORD_SRC.replace("ensure px.get' k = p.x + dx", "ensure px.get' k = p.y + dx");
    let defs = parse(&src);
    let reps = reports(&defs);
    let g = reps["move_x"].guarantees.iter().find(|g| g.clause.contains("p.y + dx")).unwrap();
    let cx = g.counterexample.clone().expect("refuted");
    assert!(cx.confirmed_by_interpreter);
    let mut d = defs.defs["move_x"].clone();
    d.examples = vec![kern::syntax::parse_example(cx.eg.trim_start_matches("eg").trim(), None).unwrap()];
    assert_eq!(d.examples[0].call.1.len(), 4, "{}", cx.eg);
    assert!(kern::check::run_examples(&defs, &d).unwrap()[0].pass, "{}", cx.eg);
}

#[test]
fn s5_a_record_is_read_by_field_or_passed_whole() {
    let cases = [
        ("g q:Pair\n  ensure 1 = 1\nbody\n  f q\n", "passes q (Pair) where a Point is wanted"),
        ("g k:Int\n  ensure 1 = 1\nbody\n  f k\n", "takes a Point here"),
        ("g k:Int\n  ensure 1 = 1\nbody\n  f (Point k)\n", "has 2 field(s), got 1"),
        ("g p:Point\n  ensure 1 = 1\nbody\n  x = p + 1\n", "p is a Point: read a field"),
        ("g p:Point\n  ensure 1 = 1\nbody\n  x = p.z\n", "has no field z"),
        ("g k:Int\n  ensure 1 = 1\nbody\n  x = Point k k\n", "only as the argument of a call"),
        ("g hold:Point\n  ensure 1 = 1\nbody\n  x = hold.x\n", "has the name of a state"),
    ];
    for (def, want) in cases {
        let src = format!("record Point x:Int y:Int\nrecord Pair a:Int b:Int\nstate hold Int\n\nf p:Point\n  ensure 1 = 1\nbody\n  x = p.x\n\n{def}");
        let e = parse_module(&src, true).expect_err(def);
        assert!(e.0.contains(want), "{def}: {}", e.0);
    }
    for (src, want) in [
        ("record Int a:Int\n", "built-in type"),
        ("record P get:Int\n", "can't be named get"),
        ("record P a:Q\n", "must be Int or Account"),
    ] {
        assert!(parse_module(src, true).unwrap_err().0.contains(want), "{src}");
    }
}

#[test]
fn s5_a_record_from_another_module_needs_an_import() {
    let root = tmp("s5-record-import");
    let shapes = ("shapes.k", "record Point x:Int y:Int\n\nstate px Int\n");
    let mv = "move_x k:Int p:Point\n  ok otherwise\n  ensure px.get' k = p.x\nbody\n  px.set k p.x\n";
    let dir = write_tree(&root.join("ok"), &[shapes, ("app.k", &format!("import shapes\n\n{mv}"))], true);
    let defs = kern::package::load(&dir.join("app.k").to_string_lossy(), &IndexMap::new()).unwrap();
    assert!(reports(&defs)["move_x"].clean);
    let dir = write_tree(&root.join("bad"), &[shapes, ("app.k", mv)], true);
    let e = kern::package::load(&dir.join("app.k").to_string_lossy(), &IndexMap::new()).unwrap_err();
    assert!(e.0.contains("move_x uses record Point, which shapes declares: add `import shapes`"), "{}", e.0);
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
fn s5_named_constants_check() {
    // closed (S5): `const NAME N` (DECISIONS.md, 2026-11-05). A constant on one side keeps `*` linear.
    must_check(
        "gap S5 named constants",
        "const fee 5
const rate 3

state paid Int

charge k:Int
  ok otherwise
  ensure paid.get' k = paid.get k + fee
body
  paid.set k (paid.get k + fee)

scaled amount:Int -> Int
  ensure result = amount * rate
body
  result = rate * amount
",
    );
}

const FEE_SRC: &str = "const fee 5

state paid Int

charge k:Int
  ok otherwise
  ensure paid.get' k = paid.get k + fee
body
  paid.set k (paid.get k + fee)
";

#[test]
fn s5_a_constant_is_its_value_to_the_checker_and_its_name_in_views() {
    let defs = parse(FEE_SRC);
    let literal = parse(&FEE_SRC.replace("const fee 5\n", "").replace("+ fee", "+ 5"));
    // the AST holds the value, so every checker path sees a literal, and the build hash (.why anchor, cache key)
    // is the literal version's
    assert_eq!(defs.defs["charge"].hash(), literal.defs["charge"].hash());
    let six = parse(&FEE_SRC.replace("const fee 5", "const fee 6"));
    assert_ne!(defs.defs["charge"].hash(), six.defs["charge"].hash(), "a new value must re-hash its users");
    // the views show the source, so they say what the name means
    let view = views::spec(&reports(&defs)["charge"], &why::Status::default());
    assert!(view.contains("\n  const fee 5\n"), "{view}");
    assert!(view.contains("paid.get k + fee"), "{view}");
}

#[test]
fn s5_a_name_that_would_hide_a_constant_is_an_error() {
    let cases = [
        ("charge fee:Int\n  ensure 1 = 1\nbody\n  x = fee\n", "the parameter fee"),
        ("charge k:Int\n  ensure 1 = 1\nbody\n  fee = k\n", "the `let` fee"),
        ("charge k:Int\n  ensure 1 = 1\nbody\n  for fee from 0 to 3\n    x = k\n", "the loop variable fee"),
        ("charge k:Int\n  ensure paid.get' k = fee'\nbody\n  paid.set k 5\n", "no after-state"),
        ("charge k:Int\n  ensure 1 = 1\nbody\n  x = fee k\n", "takes no arguments"),
        ("fee k:Int\n  ensure 1 = 1\nbody\n  x = k\n", "constant fee has the name of a definition"),
    ];
    for (def, want) in cases {
        let src = format!("const fee 5\n\nstate paid Int\n\n{def}");
        let e = parse_module(&src, true).expect_err(def);
        assert!(e.0.contains(want), "{def}: {}", e.0);
    }
}

#[test]
fn s5_a_constant_from_another_module_needs_an_import() {
    let root = tmp("s5-const-import");
    let rates = ("rates.k", "const fee 5\n\nstate paid Int\n");
    let charge = "charge k:Int\n  ok otherwise\n  ensure paid.get' k = paid.get k + fee\nbody\n  paid.set k (paid.get k + fee)\n";
    let with_import = format!("import rates\n\n{charge}");
    let dir = write_tree(&root.join("ok"), &[rates, ("app.k", &with_import)], true);
    let defs = kern::package::load(&dir.join("app.k").to_string_lossy(), &IndexMap::new()).unwrap();
    assert!(reports(&defs)["charge"].clean);
    let dir = write_tree(&root.join("bad"), &[rates, ("app.k", "import rates\n"), ("other.k", charge)], true);
    let e = kern::package::load(&dir.join("app.k").to_string_lossy(), &IndexMap::new()).unwrap_err();
    assert!(e.0.contains("uses const fee, which rates declares: add `import rates`"), "{}", e.0);
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
// S18 (closed). The page's refusal rows for body needs after a write
// =====================================================================================

/// The "Refuses when" rows of `def`'s card, as text: `condition → failure`.
fn s18_rows(src: &str, def: &str) -> Vec<String> {
    let defs = parse(src);
    let reps = reports(&defs);
    let whys: IndexMap<String, why::Status> = defs.defs.keys().map(|k| (k.clone(), why::Status::default())).collect();
    let page = kern::render::page(&reps, &defs, &whys, "s18", &kern::render::Orphans::new(), None);
    let card = &page[page.find(&format!("<h2><code>{def}</code></h2>")).unwrap_or_else(|| panic!("no card for {def}"))..];
    let table = &card[card.find("<h3>Refuses when</h3>").unwrap()..];
    let table = &table[..table.find("</table>").unwrap()];
    let strip = |s: &str| {
        let mut out = String::new();
        let mut tag = false;
        for c in s.chars() {
            match c {
                '<' => tag = true,
                '>' => tag = false,
                _ if !tag => out.push(c),
                _ => {}
            }
        }
        out.replace("&#x27;", "'").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
    };
    table.split("</tr>").map(|r| strip(&r.replace("</td><td", " </td><td"))).filter(|r| !r.trim().is_empty()).collect()
}

const S18_SRC: &str = "\
cap from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  keep ledger.total
body
  need from != to else SameAccount
  ledger.debit from amount
  ledger.credit to amount
  need ledger.balance to < 1000 else TooRich

alias from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  keep ledger.total
body
  ledger.debit from amount
  ledger.credit to amount
  need ledger.balance to < 1000 else TooRich
";

#[test]
fn s18_a_refusal_after_a_write_is_stated_in_the_pre_state_on_the_page() {
    // `from != to` is passed first, so to's new balance is its old one plus amount: one case, shown as such
    let rows = s18_rows(S18_SRC, "cap");
    assert!(rows.iter().any(|r| r.contains("balance of to + amount ≥ 1000") && r.contains("TooRich")), "{rows:?}");
    assert!(!rows.iter().any(|r| r.contains("earlier changes")), "{rows:?}");
}

#[test]
fn s18_a_refusal_whose_pre_state_form_splits_on_aliasing_says_it_reads_the_changed_state() {
    // to may be from: the pre-state form is a case split, so the row keeps the source text and says what it reads
    let rows = s18_rows(S18_SRC, "alias");
    let row = rows.iter().find(|r| r.contains("TooRich")).unwrap();
    assert!(row.contains("balance of to ≥ 1000 (after this call's earlier changes)"), "{row}");
    // a need before any write is already a pre-state condition: no note
    assert!(rows.iter().any(|r| r.contains("BadAmount") && !r.contains("earlier changes")), "{rows:?}");
}

// =====================================================================================
// S8. Mutants are body-only
// =====================================================================================

#[test]
fn s8_spec_need_mutants_are_generated() {
    // was a gap (S8): moving a `need` from the body into the spec removed its mutants. Spec needs now get their own
    // (check::spec_mutants: dropped, comparison flipped), reported apart from the body's score. Control: in the body,
    // `k > 0` is mutated.
    let body_need = parse("state cnt Int\n\nbump k:Int\n  ensure cnt.get' k = cnt.get k + 1\nbody\n  need k > 0 else BadKey\n  cnt.set k (cnt.get k + 1)\n");
    let spec_need = parse("state cnt Int\n\nbump k:Int\n  need k > 0 else BadKey\n  ok otherwise\n  ensure cnt.get' k = cnt.get k + 1\nbody\n  cnt.set k (cnt.get k + 1)\n");
    let on_need = |defs: &Module| mutants(&defs.defs["bump"]).into_iter().filter(|(d, _)| d.contains("k > 0")).count();
    assert!(on_need(&body_need) > 0, "control: no body mutant of `k > 0`");
    let n = kern::check::spec_mutants(&spec_need.defs["bump"]).into_iter().filter(|(d, _)| d.contains("k > 0")).count();
    assert_eq!(n, 2, "drop it, and `>` → `>=`");
    // the body's score keeps its meaning: spec mutants are counted apart
    let r = &analyse(&spec_need, true, None, None).unwrap()["bump"];
    assert_eq!(r.spec_mutation.as_ref().map(|m| m.total), Some(2));
}

#[test]
fn s8_a_spec_need_is_pinned_by_an_example_that_expects_the_refusal() {
    let src = "state cnt Int\n\nbump k:Int\n  need k > 0 else BadKey\n  ensure cnt.get' k = cnt.get k + 1\nbody\n  cnt.set k (cnt.get k + 1)\n";
    let score = |src: &str| analyse(&parse(src), true, None, None).unwrap()["bump"].spec_mutation.clone().unwrap();
    // nothing depends on `k > 0`: both changes survive, and the page lists them
    let m = score(src);
    assert_eq!((m.total, m.by_examples + m.by_guarantees), (2, 0), "{m:?}");
    // an example at the boundary catches both: k = 0 must refuse
    let m = score(&src.replace("body\n", "  eg | bump 0 -> BadKey\nbody\n"));
    assert_eq!((m.by_examples, m.survived.len()), (2, 0), "{m:?}");
    // a guarantee that holds only because of the need catches dropping it (the prover refutes it)
    let dep = src.replace("  ensure cnt.get' k", "  ensure k >= 1\n  ensure cnt.get' k");
    let m = score(&dep);
    assert!(m.by_guarantees >= 1 && !m.survived.iter().any(|(_, cs)| cs.iter().any(|c| c == "dropped")), "{m:?}");
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
// S11 (closed). Tier ○: a module invariant the prover can't decide, which `kern host` checks at run time
// =====================================================================================

const S11_SRC: &str = "state hold Int

invariant o:Int hold.get o >= 0

shift k:Int v:Int
  need v >= 0 else Negative
  ok otherwise
body
  hold.set (k + 1) v

drain k:Int v:Int
  ok otherwise
body
  hold.set (k + 1) (0 - v)
";

#[test]
fn s11_an_invariant_the_prover_cannot_decide_is_runtime_checked_on_the_page() {
    use kern::report::Status;
    let defs = parse(S11_SRC);
    let reps = reports(&defs);
    let inv = |n: &str| reps[n].guarantees.iter().find(|g| g.kind == "invariant").unwrap().clone();
    // a computed key: no proof and no counterexample, so ○, and the random-test count is kept
    let g = inv("shift");
    assert_eq!((g.status, kern::render::icon(g.status)), (Status::Runtime, "○"));
    assert!(g.tested.is_some());
    assert_eq!(kern::render::status(&reps["shift"]).1, "Checked at run time only, not proven");
    assert!(!reps["shift"].clean, "○ is not proven, so the definition isn't clean");
    // random tests break drain's invariant: the definition is broken, the goal itself stays ○ (no proof either way)
    assert_eq!(inv("drain").status, Status::Runtime);
    assert_eq!(kern::render::status(&reps["drain"]).1, "Broken guarantees");
    let whys: IndexMap<String, why::Status> = defs.defs.keys().map(|k| (k.clone(), why::Status::default())).collect();
    let page = kern::render::page(&reps, &defs, &whys, "s11", &kern::render::Orphans::new(), None);
    assert!(
        page.contains("<code>kern host</code> and programs from <code>kern build</code> check it after every call"),
        "the page says who checks it"
    );
    assert!(page.contains("only with <code>--no-gate</code>"), "and that the gated host doesn't serve it");
    let out = kern::render::terminal(&reps, &defs, &whys, &IndexMap::new());
    assert!(out.contains("○ invariant o:Int hold.get o >= 0   (not proven; held in"), "{out}");
}

#[test]
fn s11_the_runtime_check_the_page_promises_happens_in_the_host() {
    // what ○ claims: with the gate off (○ isn't clean), a call that breaks the invariant is undone as a fault
    let defs = parse(S11_SRC);
    let o = Options { sinks: Some(vec![]), gate: false, ..Default::default() };
    let mut h = Host::new(&defs, reports(&defs), o).unwrap();
    let r = h.call("drain", &[Value::Int(3), Value::Int(4)]).unwrap();
    assert_eq!(r["status"], "error");
    assert!(r["error"].as_str().unwrap().contains("invariant broken"), "{r:?}");
    assert_eq!(h.call("shift", &[Value::Int(3), Value::Int(4)]).unwrap()["status"], "ok");
    // and the gated host refuses to serve a definition whose best evidence is ○
    let mut gated = Host::new(&defs, reports(&defs), Options { sinks: Some(vec![]), ..Default::default() }).unwrap();
    assert!(gated.call("shift", &[Value::Int(3), Value::Int(4)]).is_err());
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
