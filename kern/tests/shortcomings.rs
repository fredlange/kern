//! Gap tests for the code-gap entries of docs/research/SHORTCOMINGS.md (S5–S12), so the register can't go stale
//! silently.
//!
//! Each `#[should_panic(expected = "gap Sn …")]` test asserts the behaviour we WANT, so today it panics with its
//! own message. An unrelated crash or a typo panics with a different message, so it doesn't count as the gap.
//! The day a gap closes, its test fails. In the same commit it becomes an ordinary test (drop `should_panic`),
//! and the entry moves to "Closed" in SHORTCOMINGS.md.
//!
//! Some gaps have no syntax yet (text, `match`, `clock.tick`, events in `eg`).
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
fn s5_a_callee_that_writes_at_a_record_field_is_havocked_at_that_field() {
    // found by tester (b18367a): `writes` entries are `state.key` or a balance's parameter, and a record field makes
    // both dotted (`hold.p.x`, `p.from`); splitting at the last dot made the prover look up a parameter `hold.p`
    let map_key = "state hold Int\n\nrecord Point x:Int y:Int\n\nput p:Point\n  ok otherwise\n  ensure hold.get' p.x = p.y\nbody\n  hold.set p.x p.y\n\nc a:Int b:Int\n  ok otherwise\n  ensure hold.get' a = b\nbody\n  put (Point a b)\n";
    let reps = reports(&parse(map_key));
    assert!(reps["c"].clean, "{:?}", reps["c"].guarantees);
    assert_eq!(kern::infer::fmt_writes(&reps["put"].writes, &reps["put"].def), "hold.p.x");
    let pay = "record Pay from:Account to:Account amount:Int

send p:Pay
  need p.amount > 0 else BadAmount
  need p.from != p.to else SameAccount
  need ledger.balance p.from >= p.amount else Insufficient
  ok otherwise
  keep ledger.total
  ensure ledger.balance' p.from = ledger.balance p.from - p.amount
  ensure ledger.balance' p.to = ledger.balance p.to + p.amount
body
  ledger.debit p.from p.amount
  ledger.credit p.to p.amount

relay p:Pay
  need as send p
  ok otherwise
  ensure ledger.balance' p.to = ledger.balance p.to + p.amount
body
  send p

direct a:Account b:Account n:Int
  need as send (Pay a b n)
  ok otherwise
  ensure ledger.balance' a = ledger.balance a - n
body
  send (Pay a b n)
";
    let reps = reports(&parse(pay));
    let dirty: Vec<&String> = reps.iter().filter(|(_, r)| !r.clean).map(|(n, _)| n).collect();
    assert!(dirty.is_empty(), "{dirty:?}");
    // record fields that are accounts are balances, not state entries
    assert_eq!(kern::infer::fmt_writes(&reps["relay"].writes, &reps["relay"].def), "p.from p.to");
    assert_eq!(views::unguaranteed_writes(&reps["relay"]), ["p.from"]);
}

#[test]
fn s5_counterexamples_carry_text_entries_and_keys_computed_from_fields() {
    // tester's findings: a refutable claim over a Text map, and over a key computed from a record field
    // (`s.lo - 1`), showed `?` because the counterexample world left those entries out (F8: a lost ✗)
    use kern::report::Status;
    let text = "state title Int -> Text\nstate mark Int\n\ng k:Int\n  ensure title.get' (k + 2) = title.get (k + 1)\nbody\n  mark.set k 1\n";
    let field = "state hold Int\nrecord Span lo:Int hi:Int\n\nagg s:Span -> Int\n  ensure result = (sum j from s.lo to s.hi of 2 * hold.get j)\nbody\n  result = (sum j from (s.lo - 1) to s.hi of 2 * hold.get j)\n";
    for (src, name) in [(text, "g"), (field, "agg")] {
        let defs = parse(src);
        let reps = reports(&defs);
        let g = reps[name].guarantees.iter().find(|g| g.kind == "ensure").unwrap();
        assert_eq!(g.status, Status::Refuted, "{name}: {:?}", g.counterexample);
        let cx = g.counterexample.clone().unwrap();
        let mut d = defs.defs[name].clone();
        d.examples = vec![kern::syntax::parse_example(cx.eg.trim_start_matches("eg").trim(), None).unwrap()];
        assert!(kern::check::run_examples(&defs, &d).unwrap()[0].pass, "{}", cx.eg);
    }
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
        ("record P a:Q\n", "must be Int, Account or Text"),
    ] {
        assert!(parse_module(src, true).unwrap_err().0.contains(want), "{src}");
    }
}

#[test]
fn s5_a_failing_eg_says_why() {
    // review of AGIZXR4NG6: a record's fields are positional in `eg`, so a grouped or short argument list failed
    // with no reason; `check` now prints what each failing example got
    let src = "state hold Int\n\nrecord Point x:Int y:Int\n\nput p:Point\n  ok otherwise\n  ensure hold.get' p.x = p.y\n  eg | put 1 -> ok | hold.1:0\n  eg | put 1 2 -> ok | hold.1:3\nbody\n  hold.set p.x p.y\n";
    let defs = parse(src);
    let reps = reports(&defs);
    let whys: IndexMap<String, why::Status> = defs.defs.keys().map(|k| (k.clone(), why::Status::default())).collect();
    let out = kern::render::terminal(&reps, &defs, &whys, &IndexMap::new());
    assert!(out.contains("failing: eg | put 1 -> ok | hold.1:0   (got error: bad call: put takes 2 argument(s), got 1)"), "{out}");
    assert!(out.contains("failing: eg | put 1 2 -> ok | hold.1:3   (got ok | hold.1:2)"), "{out}");
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
fn s5_text_checks() {
    // closed (S5): `Text`, `"…"`, `length`, `+` joins, `state NAME KEY -> Text` (DECISIONS.md, 2026-11-05, Text)
    must_check(
        "gap S5 text",
        "state title Int -> Text

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
fn s5_match_checks() {
    // closed (S5): `match E` with `PAT -> STMT` arms, desugared to the `if` chain (DECISIONS.md, 2026-11-05, match);
    // the placeholder was the chosen syntax
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
fn s5_match_is_its_if_chain_with_blocks_constants_and_text() {
    let src = "const vip 7
state mode Int
state label Int -> Text

pick src:Int t:Text
  ok otherwise
  ensure mode.get' src = 10 when src = 1
  ensure mode.get' src = 70 when src = vip
  ensure label.get' src = \"none\" when t = \"\"
  eg | pick 7 \"x\" -> ok | mode.7:70 label.7:\"x\"
  eg | pick -1 \"\" -> ok | mode.-1:3 label.-1:\"none\"
body
  match src
    1 -> mode.set src 10
    vip ->
      mode.set src 70
    -1 -> mode.set src 3
  match t
    \"\" -> label.set src \"none\"
    _ -> label.set src t
";
    must_check("match", src);
    let defs = parse(src);
    // the same tree (and so the same hash, proofs and paths) as the if chain written out
    let chain = src.replace("  match src\n    1 -> mode.set src 10\n    vip ->\n      mode.set src 70\n    -1 -> mode.set src 3\n",
        "  if src = 1\n    mode.set src 10\n  else\n    if src = vip\n      mode.set src 70\n    else\n      if src = 0 - 1\n        mode.set src 3\n")
        .replace("  match t\n    \"\" -> label.set src \"none\"\n    _ -> label.set src t\n",
        "  if t = \"\"\n    label.set src \"none\"\n  else\n    label.set src t\n");
    let by_hand = parse(&chain);
    assert_eq!(kern::syntax::flat(&defs.defs["pick"].body).len(), kern::syntax::flat(&by_hand.defs["pick"].body).len());
    // views keep the source
    assert!(views::editor(&reports(&defs), "pick", &defs.defs.keys().map(|k| (k.clone(), why::Status::default())).collect()).contains("  match src\n    1 -> mode.set src 10"));
    for (body, want) in [
        ("  match k\n    1 -> x = 1\n    1 -> x = 2\n", "appears twice"),
        ("  match k\n    _ -> x = 1\n    1 -> x = 2\n", "must be the last arm"),
        ("  match k\n    k + 1 -> x = 1\n", "names one value"),
        ("  match k\n", "needs indented arms"),
        ("  match k\n    _ -> x = 1\n", "only `_`"),
    ] {
        let e = parse_module(&format!("f k:Int\n  ensure 1 = 1\nbody\n{body}"), true).unwrap_err();
        assert!(e.0.contains(want), "{body}: {}", e.0);
    }
}

#[test]
fn s5_recursion_is_accepted() {
    // closed (S5): recursion with a termination measure, `decreases n` (DECISIONS.md, 2026-11-05, recursion)
    must_check(
        "gap S5 recursion",
        "state cnt Int

down n:Int
  need n >= 0 else Negative
  ok otherwise
  decreases n
  ensure cnt.get' 0 = cnt.get 0 + n
body
  if n > 0
    cnt.set 0 (cnt.get 0 + 1)
    down (n - 1)
",
    );
}

#[test]
fn s5_a_measure_is_checked_at_every_recursive_call() {
    use kern::report::Status;
    let src = "tri n:Int -> Int
  decreases n
  ensure result = n * (n + 1) / 2 when n >= 0
body
  if n <= 0
    result = 0
  else
    result = n + tri (n - 1)

state seen Int

ping n:Int
  need n >= 0 else Negative
  ok otherwise
  decreases 2 * n
  ensure seen.get' 0 = seen.get 0 + n
body
  if n > 0
    seen.set 0 (seen.get 0 + 1)
    pong (n - 1)

pong n:Int
  need n >= 0 else Negative
  ok otherwise
  decreases 2 * n + 1
  ensure seen.get' 0 = seen.get 0 + n
body
  ping n

spin n:Int
  need n >= 0 else Negative
  ok otherwise
  decreases n
body
  if n > 0
    spin n
";
    let defs = parse(src);
    let reps = reports(&defs);
    for n in ["tri", "ping", "pong"] {
        assert!(reps[n].clean, "{n}: {:?}", reps[n].guarantees);
        assert!(reps[n].guarantees.iter().any(|g| g.kind == "measure" && g.status == Status::Proven), "{n}");
    }
    // a measure that doesn't decrease is ✗, its counterexample recursing past the depth limit in the replay
    let g = reps["spin"].guarantees.iter().find(|g| g.kind == "measure").unwrap();
    assert_eq!(g.status, Status::Refuted, "{:?}", g);
    // and what a self-call proved rests on that measure (tester: partial correctness must not read as ✔)
    let src2 = "loop_self n:Int -> Int\n  decreases n\n  ensure result = 0\nbody\n  result = loop_self n\n";
    let r2 = reports(&parse(src2));
    let ens = r2["loop_self"].guarantees.iter().find(|g| g.kind == "ensure").unwrap();
    assert_eq!(ens.status, Status::Conditional, "{ens:?}");
    assert!(ens.blocked_by.as_ref().unwrap()[0].starts_with("termination measure"), "{ens:?}");
    // without a measure recursion is still rejected, saying what to add; a measure may not read state
    let e = analyse(&parse("f n:Int\n  ensure 1 = 1\nbody\n  f n\n"), false, None, None).unwrap_err().message();
    assert!(e.contains("recursive (f -> f); f needs `decreases E`"), "{e}");
    let e = analyse(&parse("state s Int\n\nf n:Int\n  decreases s.get n\n  ensure 1 = 1\nbody\n  f n\n"), false, None, None)
        .unwrap_err()
        .message();
    assert!(e.contains("a measure may read only parameters"), "{e}");
}

#[test]
fn s5_nested_loops_check() {
    // closed (S5): loops nest; the outer loop's frame havocs what the inner one writes (DECISIONS.md, 2026-11-05)
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
fn s5_variable_times_variable_checks() {
    // closed (S5): `*` takes two variables; Z3's nonlinear arithmetic may answer unknown (`?`), never a wrong ✔
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
fn s5_nonlinear_verdicts_agree_with_the_interpreter() {
    // the measurement the task asks for: true and false nonlinear claims; a ✔ that random tests break or a ✗ that
    // doesn't replay fails the test; undecided ones are counted (they must stay `?`/◐, never ✔)
    let claims = [
        ("result = w * h", true),
        ("result = h * w", true),
        ("result >= 0 when w >= 0 and h >= 0", true),
        ("result >= w when h >= 1 and w >= 0", true),
        ("result * result >= 0", true),
        ("(w + 1) * (w + 1) = w * w + 2 * w + 1", true),
        ("result != 7 when w > 1 and h > 1", true),
        ("result > 0 when w > 0", false),
        ("result = w * w", false),
        ("result <= w * w", false),
        ("result != 6 when w > 0 and h > 0", false),
    ];
    let (mut proven, mut refuted, mut undecided, mut true_undecided) = (0, 0, 0, 0);
    for body in ["w * h", "h * w"] {
        let ensures: String = claims.iter().map(|(c, _)| format!("  ensure {c}\n")).collect();
        let src = format!("area w:Int h:Int -> Int\n{ensures}body\n  result = {body}\n");
        let defs = parse(&src);
        let reps = reports(&defs);
        let r = &reps["area"];
        for (g, (_, right)) in r.guarantees.iter().zip(&claims) {
            match g.status {
                kern::report::Status::Proven => {
                    proven += 1;
                    assert!(*right, "a wrong claim is ✔: {}", g.clause);
                    let broke = r.props.violations.iter().any(|v| matches!(v, kern::report::Violation::Clause { clause, .. } if *clause == format!("ensure {}", g.clause)));
                    assert!(!broke, "✔ but random tests broke it: {}", g.clause);
                }
                kern::report::Status::Refuted => {
                    refuted += 1;
                    assert!(!*right, "a right claim is ✗: {}", g.clause);
                    assert!(g.counterexample.as_ref().unwrap().confirmed_by_interpreter, "{}", g.clause);
                }
                _ => {
                    undecided += 1;
                    if *right {
                        true_undecided += 1;
                    }
                    eprintln!("undecided ({:?}): {}", g.status, g.clause);
                }
            }
        }
    }
    eprintln!("nonlinear claims: {proven} proven, {refuted} refuted, {undecided} undecided ({true_undecided} of them true)");
    assert!(proven >= 8 && refuted >= 4, "proven {proven}, refuted {refuted}");
}

#[test]
fn s5_nested_loops_run_and_a_grid_seat_is_a_product() {
    // app's request #3: a booking grid, seat = row * width + col, filled by two nested loops
    let src = "state seat Int

invariant s:Int seat.get s >= 0

fill rows:Int width:Int
  need rows >= 0 and width >= 0 else Bad
  ok otherwise
  eg | fill 2 3 -> ok | seat.0:1 seat.5:1 seat.6:0
body
  for r from 0 to rows
    for c from 0 to width
      seat.set (r * width + c) 1
";
    let defs = parse(src);
    let reps = reports(&defs);
    let r = &reps["fill"];
    assert!(r.examples.iter().all(|e| e.pass), "{:?}", r.examples);
    assert!(r.guarantees.iter().all(|g| !matches!(g.status, kern::report::Status::Refuted)), "{:?}", r.guarantees);
    // a computed key (r * width + c): proven at every key, quantified (#OH54XTG7VD), so ✔
    let inv = r.guarantees.iter().find(|g| g.kind == "invariant").unwrap();
    assert_eq!(inv.status, kern::report::Status::Proven);
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
        // a compound range bound reads `cap` as applied to the words before it: say to parenthesise
        ("charge k:Int\n  ensure paid.get' j = 1 when k < fee for j from k to k + fee\nbody\n  paid.set k 1\n", "parenthesise a compound one"),
    ];
    for (def, want) in cases {
        let src = format!("const fee 5\n\nstate paid Int\n\n{def}");
        let e = parse_module(&src, true).expect_err(def);
        assert!(e.0.contains(want), "{def}: {}", e.0);
    }
}

#[test]
fn s5_a_constant_name_reads_one_way() {
    // review of UFOJU6XMZB: handler roots and declaration words are rejected, not only definitions and keywords
    for name in ["ledger", "clock", "audit", "const", "import", "state", "record", "channel", "invariant", "handler"] {
        let e = parse_module(&format!("const {name} 5\n"), true).expect_err(name);
        assert!(e.0.contains("is reserved"), "{name}: {}", e.0);
    }
    let e = parse_module("handler rand.below n:Int -> Int\n  ensure result >= 0\n\nconst rand 5\n", true).unwrap_err();
    assert!(e.0.contains("handler's root"), "{}", e.0);
    // a sum with a compound bound says so too
    let e = kern::syntax::parse_expr("sum j from lo to lo + 3 of 1", None).unwrap_err();
    assert!(e.0.contains("parenthesise a compound one"), "{}", e.0);
    // a loop with a compound bound says the same as the ensure form
    let e = parse_module("state hold Int\n\nf lo:Int\nbody\n  for k from lo to lo + 3\n    hold.set k 1\n", true).unwrap_err();
    assert!(e.0.contains("parenthesise a compound one"), "{}", e.0);
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
fn s5_import_across_packages_loads() {
    // closed (S5): kern.pkg's `use ../money` makes a sibling package's modules importable as `money.<module>`
    // (DECISIONS.md, 2026-11-05, imports across packages); the placeholder was the chosen syntax
    let gap = "gap S5 cross-package import";
    let root = tmp("s5-cross-package");
    write_tree(&root.join("money"), &[("ledger.k", &read("examples/money/ledger.k"))], true);
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

#[test]
fn s5_packages_use_packages_transitively_with_one_name_each() {
    let root = tmp("s5-pkg-deps");
    // util <- money <- app: money imports its own module and util's
    write_tree(&root.join("util"), &[("fees.k", "fee_of n:Int -> Int\n  ensure result = n / 100\nbody\n  result = n / 100\n")], true);
    write_tree(
        &root.join("money"),
        &[
            ("kern.pkg", "use ../util\n"),
            ("ledger.k", &read("examples/money/ledger.k")),
            ("pay.k", "import ledger\nimport util.fees\n\npay a:Account b:Account n:Int\n  need as transfer a b n\n  ok otherwise\n  keep ledger.total\nbody\n  transfer a b n\n"),
        ],
        false,
    );
    let app = write_tree(
        &root.join("app"),
        &[("kern.pkg", "use ../money\n\n"), ("shop.k", "import money.pay\nimport util.fees\n\nbuy a:Account b:Account\n  need as pay a b 5\n  ok otherwise\n  ensure fee_of 500 = 5\nbody\n  pay a b 5\n")],
        false,
    );
    let defs = kern::package::load(&app.join("shop.k").to_string_lossy(), &IndexMap::new()).unwrap_or_else(|e| panic!("{}", e.0));
    let pkg = defs.package.clone().unwrap();
    for m in ["shop", "money.ledger", "money.pay", "util.fees"] {
        assert!(pkg.files.contains_key(m), "{m}: {:?}", pkg.files.keys());
    }
    assert!(reports(&defs).values().all(|r| r.clean));
    // the import rule still holds across packages: shop may not use transfer without importing money.ledger
    let bad = root.join("app").join("shop.k");
    std::fs::write(&bad, "import money.pay\n\nbuy a:Account b:Account\n  ok otherwise\nbody\n  transfer a b 5\n").unwrap();
    let e = kern::package::load(&bad.to_string_lossy(), &IndexMap::new()).unwrap_err();
    assert!(e.0.contains("add `import money.ledger`"), "{}", e.0);
    // a cycle, a missing package and a bad line are load errors
    std::fs::write(root.join("util").join("kern.pkg"), "use ../app\n").unwrap();
    let e = kern::package::load(&bad.to_string_lossy(), &IndexMap::new()).unwrap_err();
    assert!(e.0.contains("cycle"), "{}", e.0);
    std::fs::write(root.join("util").join("kern.pkg"), "use ../nowhere\n").unwrap();
    assert!(kern::package::load(&bad.to_string_lossy(), &IndexMap::new()).unwrap_err().0.contains("is not a package"));
    std::fs::write(root.join("util").join("kern.pkg"), "needs money\n").unwrap();
    assert!(kern::package::load(&bad.to_string_lossy(), &IndexMap::new()).unwrap_err().0.contains("want `use ../other`"));
}

// =====================================================================================
// S6. Loops: the frame can't be stated, and there is no sum
// =====================================================================================

#[test]
fn s6_a_guarantee_kills_the_sweep_one_too_many_mutant() {
    // closed (S6's frame): batch.k's advance_subs states `keep sub_due except lo to hi` (DECISIONS.md, 2026-11-05,
    // a stated loop frame), so the `hi + 1` and `lo - 1` loop-bound mutants are caught by a guarantee.
    let defs = parse(&read("examples/batch/batch.k"));
    let m = mutation_score(&defs, &defs.defs["advance_subs"], 150);
    let bound: Vec<&String> = m.guarantee_missed.iter().map(|(d, _)| d).filter(|d| d.contains("hi + 1") || d.contains("lo - 1")).collect();
    assert!(bound.is_empty(), "gap S6 loop frame: no guarantee catches {bound:?}");
}

#[test]
fn s6_a_stated_frame_is_checked_both_ways() {
    let src = "state due Int
state title Int -> Text

sweep lo:Int hi:Int
  ok otherwise
  keep due except lo to hi
body
  for k from lo to hi
    invariant due.get' j = 0 for j from lo to k
    due.set k 0

overrun lo:Int hi:Int
  ok otherwise
  keep due except lo to hi
body
  for k from lo to (hi + 1)
    due.set k 0

touch k:Int
  ok otherwise
  keep due
body
  due.set k 1

rename k:Int t:Text
  ok otherwise
  keep title except k to (k + 1)
  keep due
body
  title.set k t
";
    let defs = parse(src);
    let reps = reports(&defs);
    let frame = |n: &str| reps[n].guarantees.iter().filter(|g| g.kind == "keep").map(|g| g.status).collect::<Vec<_>>();
    use kern::report::Status::{Proven, Refuted};
    assert_eq!(frame("sweep"), [Proven]);
    assert_eq!(frame("rename"), [Proven, Proven], "a Text map's frame, and a whole map untouched");
    for bad in ["overrun", "touch"] {
        assert_eq!(frame(bad), [Refuted], "{bad}");
        let g = reps[bad].guarantees.iter().find(|g| g.kind == "keep").unwrap();
        let cx = g.counterexample.clone().unwrap();
        assert!(cx.confirmed_by_interpreter, "{bad}: {}", cx.eg);
    }
    // wf: the state must exist, and a range needs Int keys
    for (clause, want) in [("keep nope except lo to hi", "not a declared state"), ("keep title except lo to hi", "")] {
        let src = format!("state title Int -> Text\nstate owed Account\n\nf lo:Int hi:Int\n  ok otherwise\n  {clause}\nbody\n  title.set lo \"\"\n");
        let r = analyse(&parse(&src), false, None, None);
        if want.is_empty() {
            assert!(r.is_ok(), "{clause}");
        } else {
            assert!(r.unwrap_err().message().contains(want), "{clause}");
        }
    }
    let e = analyse(&parse("state owed Account\n\nf a:Account\n  ok otherwise\n  keep owed except 0 to 1\nbody\n  owed.set a 1\n"), false, None, None).unwrap_err();
    assert!(e.message().contains("needs an Int-keyed state"), "{}", e.message());
}

#[test]
fn s6_a_sum_invariant_proves() {
    // closed (S6's sum): `sum j from A to B of E [where C]` (DECISIONS.md, 2026-11-05, aggregation). The
    // placeholder was the chosen syntax.
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

const COUNT_SRC: &str = "state status Int
state due Int

count_overdue lo:Int hi:Int -> Int
  ensure result = sum k from lo to hi of 1 where status.get k = 1 and due.get k < clock.now
  ensure result >= 0
  ensure result <= hi - lo when hi >= lo
body
  result = sum k from lo to hi of 1 where status.get k = 1 and due.get k < clock.now
";

#[test]
fn s6_apps_count_overdue_is_a_function_over_state_and_proves() {
    // .party/requests.md #1: a query over a state range, with no accumulator
    must_check("count_overdue", COUNT_SRC);
    let defs = parse(COUNT_SRC);
    // what `kern host` serves as a query: the interpreter's value for a function call
    let mut w = kern::run::World::from_example(&IndexMap::new(), 10, &IndexMap::new());
    for (k, (st, due)) in [(1, (1, 5)), (2, (1, 50)), (3, (2, 5)), (4, (1, 9))] {
        w.maps.entry("status".into()).or_default().insert(Key::Int(k), st);
        w.maps.entry("due".into()).or_default().insert(Key::Int(k), due);
    }
    let mut interp = kern::run::Interp::new(&defs);
    let r = interp.run("count_overdue", &[Value::Int(0), Value::Int(9)], &mut w).unwrap();
    assert_eq!(r, kern::run::Outcome::Ok, "the call succeeds");
    assert_eq!(interp.last_result, Some(Value::Int(2)), "tasks 1 and 4 are open and overdue at now = 10");
}

#[test]
fn s6_where_filters_a_sum_and_when_still_conditions_the_ensure() {
    use kern::syntax::{parse_clause, Expr};
    let filtered = parse_clause("r = sum j from 0 to n of x.get j where x.get j > 0", None).unwrap();
    let Expr::Bin("=", _, sum) = filtered else { panic!("{filtered:?}") };
    assert!(matches!(*sum, Expr::Sum(_, _, _, _, Some(_))));
    let conditional = parse_clause("r = sum j from 0 to n of x.get j when n > 0", None).unwrap();
    let Expr::Bin("when", eq, _) = conditional else { panic!("`when` must condition the ensure: {conditional:?}") };
    assert!(matches!(*eq, Expr::Bin("=", _, ref s) if matches!(**s, Expr::Sum(_, _, _, _, None))));
    // printed back, a sum operand is parenthesised, so the text reparses to the same tree
    let e = parse_clause("(sum j from 0 to n of x.get j) + 1 = r", None).unwrap();
    assert_eq!(parse_clause(&kern::syntax::code(&e), None).unwrap(), e);
}

const PEEK_SRC: &str = "state hold Int

peek k:Int -> Int
  ensure result = hold.get k
body
  result = hold.get k

twice k:Int
  ok otherwise
  ensure hold.get' k = hold.get k + 1
body
  a = peek k
  hold.set k (a + 1)
  b = peek k
  hold.set k b
";

#[test]
fn s6_a_function_that_reads_state_is_a_function_of_that_state() {
    // one Int-only symbol for `peek k` would make `a` and `b` equal and the path contradictory (∅, or a vacuous ✔)
    let reps = reports(&parse(PEEK_SRC));
    assert!(reps["twice"].clean && reps["twice"].guarantees.iter().all(|g| g.status == kern::report::Status::Proven));
    let wrong = reports(&parse(&PEEK_SRC.replace("hold.get k + 1\nbody\n  a", "hold.get k + 2\nbody\n  a")));
    let g = wrong["twice"].guarantees.iter().find(|g| g.clause.contains("+ 2")).unwrap();
    assert_eq!(g.status, kern::report::Status::Refuted);
    // still no writes: a function that changes state is rejected
    let e = analyse(&parse("state hold Int\n\nf k:Int -> Int\n  ensure result = 0\nbody\n  hold.set k 1\n  result = 0\n"), false, None, None);
    assert!(e.unwrap_err().message().contains("may read state but not change it"));
}

#[test]
fn s6_a_need_on_a_sum_after_a_write_is_not_stated_in_the_pre_state() {
    // the sum's reads range over j, so they can't be rewritten through `hold.set k 5` key by key: no lifted row
    let src = "state hold Int\n\ngrab k:Int\n  ensure 1 = 1\nbody\n  hold.set k 5\n  need (sum j from 0 to 3 of hold.get j) < 10 else Full\n";
    let reps = reports(&parse(src));
    assert!(!reps["grab"].lifted.iter().any(|l| l.ctor == "Full"), "{:?}", reps["grab"].lifted);
    // before any write it is lifted unchanged
    let before = src.replace("  hold.set k 5\n  need (sum j from 0 to 3 of hold.get j) < 10 else Full\n", "  need (sum j from 0 to 3 of hold.get j) < 10 else Full\n  hold.set k 5\n");
    assert!(reports(&parse(&before))["grab"].lifted.iter().any(|l| l.ctor == "Full"));
}

#[test]
fn s6_sum_verdicts_agree_with_the_interpreter() {
    // the floor for sums (neither the fuzzer nor the digests generate them): seeded programs whose claims are right
    // or slightly wrong; a ✔ with a random-test violation, or a ✗ that doesn't replay, is a contradiction
    let bodies = ["hold.get j", "2 * hold.get j", "hold.get j + j", "1", "j"];
    let filters = ["", " where hold.get j > 0", " where j < lo + 2", " where hold.get j = j"];
    let tweaks = ["", " + 1", " - (sum j from lo to hi of 1)"];
    let (mut proven, mut refuted, mut other) = (0, 0, 0);
    for (n, ((b, f), t)) in bodies
        .iter()
        .flat_map(|b| filters.iter().map(move |f| (b, f)))
        .flat_map(|bf| tweaks.iter().map(move |t| (bf, t)))
        .enumerate()
    {
        // the sum lemma's claims (#IHGVAGXX6Q): right for a 0/1 term, wrong for others; they don't depend on t
        let bounds = if t.is_empty() { "\n  ensure result >= 0\n  ensure result <= hi - lo when hi >= lo" } else { "" };
        let src = format!(
            "state hold Int\n\nf lo:Int hi:Int -> Int\n  ensure result = (sum j from lo to hi of {b}{f}){t}{bounds}\nbody\n  result = sum j from lo to hi of {b}{f}\n\nsweep pool:Account lo:Int hi:Int\n  ok otherwise\n  ensure ledger.balance' pool = ledger.balance pool + (sum j from lo to hi of {b}{f}){t}\nbody\n  for k from lo to hi\n    invariant ledger.balance' pool = ledger.balance pool + (sum j from lo to k of {b}{f})\n    ledger.credit pool (sum j from k to (k + 1) of {b}{f})\n"
        );
        let defs = parse_module(&src, true).unwrap_or_else(|e| panic!("{n}: {}\n{src}", e.0));
        let reps = analyse(&defs, false, None, None).unwrap_or_else(|e| panic!("{n}: {}\n{src}", e.message()));
        for r in reps.values() {
            for g in &r.guarantees {
                match g.status {
                    kern::report::Status::Proven => {
                        proven += 1;
                        let broke = r.props.violations.iter().any(|v| match v {
                            kern::report::Violation::Clause { clause, .. } => *clause == format!("{} {}", g.kind, g.clause),
                            _ => true,
                        });
                        assert!(!broke, "{n}: proven, but random tests broke it:\n{src}\n{:?}", r.props.violations)
                    }
                    kern::report::Status::Refuted => {
                        refuted += 1;
                        assert!(g.counterexample.as_ref().unwrap().confirmed_by_interpreter, "{n}: unconfirmed ✗\n{src}")
                    }
                    _ => {
                        other += 1;
                        eprintln!("{n}: {:?} {} {}", g.status, g.kind, g.clause);
                    }
                }
            }
        }
    }
    eprintln!("sum programs: {} goals proven, {} refuted, {} other", proven, refuted, other);
    assert!(proven > 60 && refuted > 30, "proven {proven}, refuted {refuted}: the set must exercise both");
    // every goal here is decidable: an undecided one found a bug once (a free `lo` and the lower bound shared a Z3
    // name, so the sum read its bound as `lo`)
    assert_eq!(other, 0, "undecided goals (listed above)");
}

const TEXT_SRC: &str = "state title Int -> Text
channel console

name_it slot:Int t:Text
  need length t > 0 else Empty
  ok otherwise
  ensure title.get' slot = t
  eg | name_it 1 \"hello | world\" -> ok | title.1:\"hello | world\"
  eg title.1:\"old\" | name_it 1 \"\" -> Empty
body
  title.set slot t

greet k:Int name:Text
  need length name <= 20 else TooLong
  ok otherwise
  ensure title.get' k = \"Hi \" + name
  ensure length (title.get' k) = length name + 3
  emit console Line (\"Hi \" + name)
body
  msg = \"Hi \" + name
  title.set k msg
  console.emit (Line msg)

clear lo:Int hi:Int
  ok otherwise
  ensure title.get' j = \"\" for j from lo to hi
body
  for k from lo to hi
    invariant title.get' j = \"\" for j from lo to k
    title.set k \"\"
";

#[test]
fn s5_text_proves_joins_lengths_loops_and_events_and_keeps_its_source() {
    must_check("text", TEXT_SRC);
    let defs = parse(TEXT_SRC);
    // a literal with `|` and spaces is one argument, in source, eg lines and views
    let ex = &defs.defs["name_it"].examples[0];
    assert_eq!(ex.call.1, [Key::Int(1), Key::Str("hello | world".into())]);
    assert_eq!(ex.after_texts["title"][&Key::Int(1)], "hello | world");
    let view = views::editor(&reports(&defs), "name_it", &defs.defs.keys().map(|k| (k.clone(), why::Status::default())).collect());
    assert!(view.contains("eg | name_it 1 \"hello | world\" -> ok | title.1:\"hello | world\""), "{view}");
    assert!(views::spec(&reports(&defs)["greet"], &why::Status::default()).contains("ensure title.get' k = \"Hi \" + name"));
}

#[test]
fn s5_a_wrong_text_claim_is_refuted_by_an_eg_that_replays() {
    // F8 for Text: the counterexample quotes Text arguments and entries, so it parses back to the same call
    let src = "state title Int -> Text\n\nwrong k:Int name:Text\n  ok otherwise\n  ensure title.get' k = name\nbody\n  title.set k (name + \"!\")\n";
    let defs = parse(src);
    let reps = reports(&defs);
    let g = &reps["wrong"].guarantees.iter().find(|g| g.kind == "ensure").unwrap();
    assert_eq!(g.status, kern::report::Status::Refuted);
    let cx = g.counterexample.clone().unwrap();
    assert!(cx.confirmed_by_interpreter, "{}", cx.eg);
    let mut d = defs.defs["wrong"].clone();
    d.examples = vec![kern::syntax::parse_example(cx.eg.trim_start_matches("eg").trim(), None).unwrap()];
    assert!(kern::check::run_examples(&defs, &d).unwrap()[0].pass, "{}", cx.eg);
}

#[test]
fn s5_text_is_typed_apart_from_int_and_account() {
    let head = "state title Int -> Text\nstate n Int\n\n";
    for (def, want) in [
        ("f t:Text\n  ensure 1 = 1\nbody\n  x = t + 1\n", "needs Text and Text"),
        ("f t:Text\n  ensure 1 = 1\nbody\n  n.set 1 t\n", "must be Int"),
        ("f k:Int\n  ensure 1 = 1\nbody\n  title.set k 5\n", "must be Text"),
        ("f t:Text\n  ensure 1 = 1\nbody\n  need t < \"b\" else Bad\n", "needs Int and Int"),
        ("f t:Text\n  ensure 1 = 1\nbody\n  need length t > 0 else Bad t\n", "payload"),
        ("f k:Int\n  ensure 1 = 1\nbody\n  x = length k\n", "must be Text"),
        ("length k:Int\n  ensure 1 = 1\nbody\n  x = k\n", "`length` is built in"),
    ] {
        let defs = parse_module(&format!("{head}{def}"), true).unwrap();
        let e = analyse(&defs, false, None, None).unwrap_err().message();
        assert!(e.contains(want), "{def}: {e}");
    }
    // a refusal payload in an eg is Int only
    let e = parse_module("f t:Text\n  ensure 1 = 1\n  eg | f \"a\" -> Bad \"x\"\nbody\n  x = t\n", true).unwrap_err();
    assert!(e.0.contains("payload is Int only"), "{}", e.0);
}

#[test]
fn s5_text_verdicts_agree_with_the_interpreter() {
    // the floor for Text (neither the fuzzer nor the digests generate it): seeded programs whose claims are right
    // or slightly wrong; a ✔ with a random-test violation, a ✗ that doesn't replay, or an undecided goal fails
    let values = ["t", "t + \"!\"", "\"x\" + t", "t + t", "\"\"", "u + t"];
    let claims = [
        ("title.get' k = {v}", true),
        ("title.get' k = t", false),
        ("length (title.get' k) = length ({v})", true),
        ("length (title.get' k) >= length t", false),
        ("length (title.get' k) <= length t + length u + 1", false),
        ("title.get' k != \"zz\" when length t < 2 and length u < 2", false),
        ("title.get' k = u + t", false),
        ("length (title.get' k) < 4", false),
    ];
    let (mut proven, mut refuted, mut other) = (0, 0, 0);
    for (n, v) in values.iter().enumerate() {
        let ensures: String = claims.iter().map(|(c, _)| format!("  ensure {}\n", c.replace("{v}", v))).collect();
        let src = format!(
            "state title Int -> Text\n\nput k:Int t:Text u:Text\n  ok otherwise\n{ensures}body\n  title.set k ({v})\n\nstore k:Int t:Text u:Text\n  ok otherwise\n  ensure title.get' k = {v}\nbody\n  title.set k ({v})\n\nkeep_other k:Int j:Int t:Text u:Text\n  need j != k else Same\n  ok otherwise\n  ensure title.get' j = title.get j\n  ensure title.get' k = {v}\nbody\n  store k t u\n"
        );
        let defs = parse_module(&src, true).unwrap_or_else(|e| panic!("{n}: {}\n{src}", e.0));
        let reps = analyse(&defs, false, None, None).unwrap_or_else(|e| panic!("{n}: {}\n{src}", e.message()));
        for r in reps.values() {
            for g in &r.guarantees {
                match g.status {
                    kern::report::Status::Proven => {
                        proven += 1;
                        let broke = r.props.violations.iter().any(|x| match x {
                            kern::report::Violation::Clause { clause, .. } => *clause == format!("{} {}", g.kind, g.clause),
                            _ => true,
                        });
                        assert!(!broke, "{n}: proven, but random tests broke it:\n{src}\n{:?}", r.props.violations)
                    }
                    kern::report::Status::Refuted => {
                        refuted += 1;
                        assert!(g.counterexample.as_ref().unwrap().confirmed_by_interpreter, "{n}: unconfirmed ✗\n{src}")
                    }
                    _ => {
                        other += 1;
                        eprintln!("{n}: {:?} {} {}", g.status, g.kind, g.clause);
                    }
                }
            }
        }
    }
    eprintln!("text programs: {proven} goals proven, {refuted} refuted, {other} other");
    assert!(proven > 20 && refuted > 10, "proven {proven}, refuted {refuted}: the set must exercise both");
    assert_eq!(other, 0, "undecided goals (listed above)");
}

#[test]
fn an_invariant_over_a_computed_key_is_proven_at_every_key() {
    // app's booking #1 and shop 0b: a write at room * 7 + day, or at a key read from state, left the invariant ○
    // and every caller's guarantees `!`; now it is proven quantified (the invariant holds everywhere before)
    use kern::report::Status;
    let src = "state slot Int
state item_of Int

invariant s:Int slot.get s >= 0

put room:Int day:Int guest:Int
  need guest > 0 else NoGuest
  ok otherwise
  ensure slot.get' (room * 7 + day) = guest
body
  slot.set (room * 7 + day) guest

bump order:Int
  ok otherwise
body
  slot.set (item_of.get order) (slot.get (item_of.get order) + 1)

clear_day day:Int
  ok otherwise
  ensure slot.get' (r * 7 + day) = 0 for r from 0 to 5
body
  for r from 0 to 5
    invariant slot.get' (q * 7 + day) = 0 for q from 0 to r
    slot.set (r * 7 + day) 0

book room:Int day:Int guest:Int
  need guest > 0 else NoGuest
  ok otherwise
  ensure slot.get' (room * 7 + day) = guest
body
  put room day guest
";
    let defs = parse(src);
    let reps = reports(&defs);
    for n in ["put", "bump", "clear_day", "book"] {
        assert!(reps[n].clean, "{n}: {:?}", reps[n].guarantees);
    }
    // a write that breaks it is ✗ (in a module of its own: a definition that breaks an invariant means no
    // definition may assume it, so it would downgrade the others)
    let wipe = "state slot Int\n\ninvariant s:Int slot.get s >= 0\n\nwipe room:Int day:Int\n  ok otherwise\nbody\n  slot.set (room * 7 + day) (0 - 1)\n";
    let reps = reports(&parse(wipe));
    let g = reps["wipe"].guarantees.iter().find(|g| g.kind == "invariant").unwrap();
    assert_eq!(g.status, Status::Refuted);
    assert!(g.counterexample.as_ref().unwrap().confirmed_by_interpreter);
}

#[test]
fn random_tests_skip_worlds_that_break_a_module_invariant() {
    // app's request 0 (shop): `cancel` relies on the invariant `phase.get o = 0 or item_of.get o > 0`; random
    // worlds broke it, the call refused, and that counted as a violation of a proven `ok otherwise`
    let src = "state phase Int
state item_of Int
state stock Int

invariant o:Int phase.get o = 0 or item_of.get o > 0

restock item:Int
  need item > 0 else NoSuchItem
  ok otherwise
  ensure stock.get' item = stock.get item + 1
body
  stock.set item (stock.get item + 1)

cancel order:Int
  need phase.get order = 1 else CannotCancel
  ok otherwise
  ensure phase.get' order = 4
body
  restock (item_of.get order)
  phase.set order 4
";
    let reps = reports(&parse(src));
    let r = &reps["cancel"];
    assert!(r.guarantees.iter().all(|g| g.status == kern::report::Status::Proven), "{:?}", r.guarantees);
    assert!(r.props.violations.is_empty(), "{:?}", r.props.violations);
    assert!(r.clean);
}

#[test]
fn mutation_testing_of_a_quantified_text_loop_finishes() {
    // app's booking #4: a full check ran 15+ minutes, Z3's Diophantine handler (lp.dio) growing bignums on a mutant's
    // loop goal without checking the step limit; with it off (zz::setup) the same check takes seconds
    let src = "const rooms 5
const days 7
const slots 35

state slot Int
state holder Int -> Text

clear_day day:Int
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  ok otherwise
  ensure slot.get' (r * days + day) = 0 for r from 0 to rooms
  ensure holder.get' (r * days + day) = \"\" for r from 0 to rooms
  ensure slot.get' s = slot.get s when s - (s / days) * days != day for s from 0 to slots
body
  for r from 0 to rooms
    invariant slot.get' (q * days + day) = 0 for q from 0 to r
    invariant holder.get' (q * days + day) = \"\" for q from 0 to r
    invariant slot.get' s = slot.get s when s - (s / days) * days != day or s / days >= r for s from 0 to slots
    holder.set (r * days + day) \"\"
    slot.set (r * days + day) 0
";
    let defs = parse(src);
    let t = std::time::Instant::now();
    let m = mutation_score(&defs, &defs.defs["clear_day"], 150);
    assert!(m.total >= 10, "{m:?}");
    eprintln!("clear_day: {} mutants scored in {:?}", m.total, t.elapsed());
}

// =====================================================================================
// S7. Refusal conditions after a write// =====================================================================================
// S7. Refusal conditions after a write// =====================================================================================
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
fn s10_eg_can_assert_events() {
    // closed (S10): a fourth `|` section lists the events (DECISIONS.md, 2026-11-05, S10); the placeholder was the
    // chosen syntax. An example expecting the wrong event fails
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
fn s10_one_call_can_see_two_instants() {
    // closed (S10): `clock.tick` moves the call's time on by exactly 1; the placeholder was the chosen syntax
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

#[test]
fn s10_eg_events_are_exact_per_channel_and_say_what_was_appended() {
    let src = "channel console

greet a:Account n:Int
  ok otherwise
  emit Hi a n
  emit console Line \"hi\"
  eg A:1@u1 | greet A 3 -> ok | | Hi A 3, console Line \"hi\"
  eg A:1@u1 | greet A 3 -> ok | | Hi A 3
  eg A:1@u1 | greet A 3 -> ok | | console Line \"hi\"
  eg A:1@u1 | greet A 3 -> ok | | Hi A 4
  eg A:1@u1 | greet A 3 -> ok | |
body
  audit.emit (Hi a n)
  console.emit (Line \"hi\")
";
    let defs = parse(src);
    let ex = kern::check::run_examples(&defs, &defs.defs["greet"]).unwrap();
    let pass: Vec<bool> = ex.iter().map(|e| e.pass).collect();
    // both channels named; audit only (console unconstrained); console only (audit then must be empty: fails);
    // a wrong argument; an empty section (nothing on audit: fails)
    assert_eq!(pass, [true, true, false, false, false], "{ex:?}");
    assert_eq!(ex[3].got, "ok, appending Hi A 3");
}

#[test]
fn s10_a_tick_is_one_instant_later_and_a_need_after_it_is_not_stated_at_the_start() {
    let src = "state seen Int

stamp k:Int
  ok otherwise
  ensure seen.get' k = clock.now + 1
  eg now:5 | stamp 1 -> ok | seen.1:6
body
  clock.tick
  seen.set k clock.now

twice k:Int
  ok otherwise
  ensure seen.get' k >= clock.now + 1
body
  stamp k

late d:Int
  emit Stamped clock.now
body
  audit.emit (Stamped clock.now)
  clock.tick
  need clock.now <= d else TooLate
";
    let defs = parse(src);
    let reps = reports(&defs);
    assert!(reps["stamp"].clean && reps["twice"].clean, "{:?} {:?}", reps["stamp"].guarantees, reps["twice"].guarantees);
    // `need clock.now <= d` after a tick is not `clock.now <= d` at the call's start: no lifted row says so
    assert!(!reps["late"].lifted.iter().any(|l| l.ctor == "TooLate"), "{:?}", reps["late"].lifted);
    assert!(views::iface(&reps["late"], &why::Status::default()).contains("uses clock audit clock.tick"));
    // a refused call's tick is undone with it, and a function may not tick
    let mut w = kern::run::World::from_example(&IndexMap::new(), 7, &IndexMap::new());
    let out = kern::run::Interp::new(&defs).run("late", &[Value::Int(0)], &mut w).unwrap();
    // (after every call the clock moves on once, refused calls included: 7 -> 8, not 9)
    assert!(matches!(out, kern::run::Outcome::Fail(ref c, _) if c == "TooLate"), "{out:?}");
    assert_eq!((w.clock, w.audit.len()), (8, 0));
    let e = analyse(&parse("f k:Int -> Int\n  ensure result = 0\nbody\n  clock.tick\n  result = 0\n"), false, None, None).unwrap_err();
    assert!(e.message().contains("may read state but not change it"), "{}", e.message());
}

// =====================================================================================
// S11 (closed). Tier ○: a module invariant the prover can't decide, which `kern host` checks at run time
// =====================================================================================

// a³ + b³ - c³ is never 0 for positive a, b, c (Fermat, n = 3): true, and beyond what Z3 decides. Computed keys
// alone no longer give ○ (#OH54XTG7VD proves those invariants quantified).
const S11_SRC: &str = "state hold Int

invariant o:Int hold.get o != 0

cube k:Int a:Int b:Int c:Int
  need a > 0 and b > 0 and c > 0 else Bad
  ok otherwise
body
  hold.set (k + 1) (a * a * a + b * b * b - c * c * c)

drain k:Int v:Int
  ok otherwise
body
  hold.set (k + 1) 0
";

#[test]
fn s11_an_invariant_the_prover_cannot_decide_is_runtime_checked_on_the_page() {
    use kern::report::Status;
    let defs = parse(S11_SRC);
    let reps = reports(&defs);
    let inv = |n: &str| reps[n].guarantees.iter().find(|g| g.kind == "invariant").unwrap().clone();
    // no proof and no counterexample, so ○, and the random-test count is kept
    let g = inv("cube");
    assert_eq!((g.status, kern::render::icon(g.status)), (Status::Runtime, "○"));
    assert!(g.tested.is_some());
    assert_eq!(kern::render::status(&reps["cube"]).1, "Checked at run time only, not proven");
    assert!(!reps["cube"].clean, "○ is not proven, so the definition isn't clean");
    // drain breaks it: ✗ with a counterexample that replays, never ○ (reviewer)
    let g = inv("drain");
    assert_eq!(g.status, Status::Refuted);
    let cx = g.counterexample.clone().unwrap();
    assert!(cx.confirmed_by_interpreter && cx.eg.contains("-> ok"), "{}", cx.eg);
    let mut d = defs.defs["drain"].clone();
    d.examples = vec![kern::syntax::parse_example(cx.eg.trim_start_matches("eg").trim(), None).unwrap()];
    assert!(kern::check::run_examples(&defs, &d).unwrap()[0].pass, "{}", cx.eg);
    assert_eq!(kern::render::status(&reps["drain"]).1, "Broken guarantees");
    let whys: IndexMap<String, why::Status> = defs.defs.keys().map(|k| (k.clone(), why::Status::default())).collect();
    let page = kern::render::page(&reps, &defs, &whys, "s11", &kern::render::Orphans::new(), None);
    assert!(
        page.contains("<code>kern host</code> and programs from <code>kern build</code> check it after every call"),
        "the page says who checks it"
    );
    assert!(page.contains("only with <code>--no-gate</code>"), "and that the gated host doesn't serve it");
    let out = kern::render::terminal(&reps, &defs, &whys, &IndexMap::new());
    assert!(out.contains("○ invariant o:Int hold.get o != 0   (not proven; held in"), "{out}");
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
    let one = [Value::Int(3), Value::Int(1), Value::Int(1), Value::Int(1)];
    assert_eq!(h.call("cube", &one).unwrap()["status"], "ok");
    // and the gated host refuses to serve a definition whose best evidence is ○
    let mut gated = Host::new(&defs, reports(&defs), Options { sinks: Some(vec![]), ..Default::default() }).unwrap();
    assert!(gated.call("cube", &one).is_err());
}

// =====================================================================================
// S21 (closed by runtime's 5ffab2d). A huge loop or sum bound ends as a fault, not a held host
// =====================================================================================

const S21_SRC: &str = "state hold Int

count n:Int
  ok otherwise
body
  hold.set 0 (sum j from 0 to n of 1)

fill n:Int
  ok otherwise
body
  for k from 0 to n
    hold.set 1 k
";

#[test]
fn s21_a_huge_bound_ends_as_a_fault_in_the_host_and_the_compiled_program() {
    // `ok otherwise` is ✔ (the proof says the call succeeds, not when); at run time the call stops at the
    // per-call iteration limit as an atomic fault, at once, in both runtimes
    let defs = parse(S21_SRC);
    let reps = reports(&defs);
    assert!(reps["count"].clean && reps["fill"].clean);
    let mut h = Host::new(&defs, reps, Options { sinks: Some(vec![]), ..Default::default() }).unwrap();
    h.init("").unwrap();
    let t = std::time::Instant::now();
    for (name, n) in [("count", i64::MAX as i128), ("fill", 1_000_000_000_000)] {
        let r = h.call(name, &[Value::Int(n)]).unwrap();
        assert_eq!((r["status"].as_str(), r["fault"].as_str()), (Some("error"), Some("limit")), "{name}: {r:?}");
    }
    assert!(t.elapsed().as_secs() < 5, "the host was held for {:?}", t.elapsed());
    assert_eq!(h.call("count", &[Value::Int(5)]).unwrap()["status"], "ok", "a small bound still runs");
    assert!(h.load().unwrap().maps["hold"].get(&Key::Int(1)).is_none(), "the faulted fill wrote nothing");
    if std::process::Command::new("cargo").arg("--version").output().is_err() {
        return eprintln!("SKIP compiled half: needs cargo");
    }
    let dir = tmp("s21");
    let file = dir.join("s21.k");
    std::fs::write(&file, S21_SRC).unwrap();
    let crate_dir = kern::emit_rust::emit(&file.to_string_lossy(), Some(&dir.join("build").to_string_lossy())).unwrap();
    let binary = kern::emit_rust::build(&crate_dir).unwrap();
    let input = format!("world now:1\ncall count {}\ncall fill 1000000000000\ncall count 5\ndump\n", i64::MAX);
    let t = std::time::Instant::now();
    let out = {
        use std::io::Write;
        let mut c = std::process::Command::new(&binary)
            .arg("run")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap()
    };
    assert!(t.elapsed().as_secs() < 5, "the compiled program was held for {:?}", t.elapsed());
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("fault") && lines[1].starts_with("fault") && lines[2].starts_with("ok"), "{out}");
    assert!(lines[3].ends_with("hold.0:5"), "only the small call left a trace: {out}");
}

// =====================================================================================
// S20 (lang's half). Integer literals: one range, i64, refused where they are written
// =====================================================================================

#[test]
fn s20_an_integer_literal_outside_i64_is_refused_where_it_is_written() {
    // the host's store and `kern build` programs are i64 (runtime's half: values at the host boundary); a literal
    // past it used to parse and run in the interpreter's i128
    let big = "99999999999999999999999";
    let max = i64::MAX.to_string();
    let over = (i64::MAX as i128 + 1).to_string();
    let body = |lit: &str| format!("state hold Int\n\nput k:Int\n  ok otherwise\nbody\n  hold.set k {lit}\n");
    for lit in [big, over.as_str()] {
        let e = parse_module(&body(lit), true).unwrap_err();
        assert!(e.0.contains("outside Int's range"), "{lit}: {}", e.0);
        let e = parse_module(&format!("const c {lit}\n"), true).unwrap_err();
        assert!(e.0.contains("outside Int's range"), "const {lit}: {}", e.0);
        for eg in [format!("| put {lit} -> ok"), format!("hold.1:{lit} | put 1 -> ok"), format!("| put 1 -> Bad {lit}")] {
            let src = body("1").replace("  ok otherwise\n", &format!("  ok otherwise\n  eg {eg}\n"));
            let e = parse_module(&src, true).unwrap_err();
            assert!(e.0.contains("outside Int's range"), "eg {eg}: {}", e.0);
            assert!(e.0.starts_with("line 5: "), "the eg's line (reviewer): {}", e.0);
        }
    }
    // the largest i64 is a literal like any other
    assert!(must_parse_and_check(&body(&max)));
}

fn must_parse_and_check(src: &str) -> bool {
    analyse(&parse(src), false, None, None).is_ok()
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
fn s12_a_request_with_no_reply_times_out_into_a_call() {
    // closed (S12, runtime's f79f085): examples/limits asks for A's limit; the network never answers. An hour per
    // call later, the request has timed out into its `fail` call (`limit_failed`), once, freeing `pending`.
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
    assert!(pending == Some(0), "S12 request timeout: after 24 hours pending.A = {pending:?}, reply calls run: {ran:?}");
    assert_eq!(ran, ["limit_failed"], "the timeout fires exactly once");
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
