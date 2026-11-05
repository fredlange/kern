//! Adversarial tests against docs/FUNDAMENTALS.md (port of tests/test_fundamentals.py, the front-end half): F4
//! (derive, don't declare), F6 (why), F7 (closed world), F9 (tokens), the incidental parse tests, the parse and type
//! errors of the scale and roadmap sections, the ratchet (S1) and the cache (S2). The prover and checker sections
//! are kern-8b's port.

mod common;
use common::{tmp, ROOT};

use std::collections::BTreeSet;

use indexmap::IndexMap;
use kern::baseline;
use kern::infer::{callees, fails, uses};
use kern::pyrandom::Random;
use kern::report::{analyse, Report};
use kern::run::{Interp, Outcome, RunError, Value, World};
use kern::syntax::{code, parse_expr, parse_module, Expr, Module, Need};
use kern::why;
use regex::Regex;

fn read(p: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/{p}")).unwrap()
}

fn v4() -> String {
    read("examples/ledger-tour/ledger_v4.k")
}

fn large() -> String {
    read("examples/ledger-large/ledger_large.k")
}

fn escrow1() -> String {
    read("examples/escrow/escrow_s1.k")
}

const STATE_HEAD: &str = "state hold Int\nstate frozen Account\ninvariant a:Account ledger.balance a >= 0\n\n";

fn parse(src: &str) -> Module {
    parse_module(src, true).unwrap()
}

fn wf_error(src: &str) -> String {
    match parse_module(src, true) {
        Err(e) => e.0,
        Ok(defs) => kern::wf::check_module(&defs).err().map(|e| e.message()).unwrap_or_default(),
    }
}

fn analysed(src: &str, mutation: bool) -> (Module, IndexMap<String, Report>) {
    let defs = parse(src);
    let reps = analyse(&defs, mutation, None, None).unwrap();
    (defs, reps)
}

fn no_whys(defs: &Module) -> IndexMap<String, why::Status> {
    defs.defs.keys().map(|n| (n.clone(), why::Status::default())).collect()
}

fn random_world(rnd: &mut Random) -> World {
    let mut w = World::default();
    for a in ["A", "B", "C"] {
        w.bal.insert(a.into(), rnd.randint(0, 300) as i128);
    }
    for a in ["A", "B", "C"] {
        w.own.insert(a.into(), rnd.choice(&["u1", "u2"]).to_string());
    }
    w
}

fn random_args(rnd: &mut Random, d: &kern::syntax::Def) -> Vec<Value> {
    d.params
        .iter()
        .map(|(_, t)| {
            if t == "Account" {
                Value::Str(rnd.choice(&["A", "B", "C"]).to_string())
            } else {
                Value::Int(rnd.randint(-3, 200) as i128)
            }
        })
        .collect()
}

// ===================================================================================== F4. Derive, don't declare

#[test]
fn f4_observed_effects_and_failures_are_inferred() {
    let defs = parse(&large());
    let mut rnd = Random::from_int(3);
    for (n, d) in &defs.defs {
        let (mut observed, mut failed) = (BTreeSet::<String>::new(), BTreeSet::<String>::new());
        for _ in 0..300 {
            let mut w = random_world(&mut rnd);
            let args = random_args(&mut rnd, d);
            let mut it = Interp::new(&defs);
            it.effect_log = Some(vec![]);
            if let Outcome::Fail(c, _) = it.run(n, &args, &mut w).unwrap() {
                failed.insert(c);
            }
            observed.extend(it.effect_log.unwrap().into_iter().map(String::from));
        }
        if observed.contains("ledger.write") {
            observed.remove("ledger.read");
        }
        assert!(observed.is_subset(&uses(&defs, d)), "{n}: {observed:?}");
        let inferred: BTreeSet<String> = fails(&defs, d).into_keys().collect();
        assert!(failed.is_subset(&inferred), "{n}: {failed:?}");
    }
}

#[test]
fn f4_inference_is_transitive_through_four_levels() {
    let defs = parse(&large());
    let d = &defs.defs["month_end"]; // month_end -> settle -> payout -> transfer
    assert_eq!(uses(&defs, d), ["ledger.write", "clock", "audit"].iter().map(|s| s.to_string()).collect());
    let f: BTreeSet<String> = fails(&defs, d).into_keys().collect();
    let want: BTreeSet<String> =
        ["FeeTooLarge", "BadAmount", "SameAccount", "SameOwner", "Insufficient"].iter().map(|s| s.to_string()).collect();
    assert_eq!(f, want);
}

#[test]
fn f4_handler_inside_constructor_inside_failure_payload_is_inferred() {
    let defs = parse("f a:Account n:Int\nbody\n  need n > 0 else Late (clock.now)\n  ledger.credit a n\n");
    assert_eq!(uses(&defs, &defs.defs["f"]), ["ledger.write", "clock"].iter().map(|s| s.to_string()).collect());
}

#[test]
fn f4_bad_calls_are_rejected_before_anything_runs() {
    // an unknown account is a bad *call*, rejected at the boundary; inside a body it cannot arise (types)
    let defs = parse(&v4());
    let mut w = World::new(IndexMap::from([("A".to_string(), 500)]), IndexMap::from([("A".to_string(), "u1".to_string())]), 0);
    let snap = w.clone();
    let s = |x: &str| Value::Str(x.into());
    for args in [vec![s("A"), s("Z"), Value::Int(10)], vec![s("A"), s("A"), s("10")], vec![s("A"), Value::Int(5), Value::Int(10)], vec![s("A"), s("A")]] {
        match Interp::new(&defs).run("transfer", &args, &mut w) {
            Err(RunError::Kern(m)) => assert!(m.contains("bad call"), "{m}"),
            other => panic!("{args:?}: {other:?}"),
        }
        assert!(w.bal == snap.bal && w.audit == snap.audit && w.clock == snap.clock);
    }
}

#[test]
fn f4_every_outcome_of_a_valid_call_is_ok_or_an_inferred_failure() {
    let defs = parse(&large());
    let mut rnd = Random::from_int(5);
    for (n, d) in &defs.defs {
        let f = fails(&defs, d);
        for _ in 0..200 {
            let mut w = random_world(&mut rnd);
            let args = random_args(&mut rnd, d);
            match Interp::new(&defs).run(n, &args, &mut w).unwrap() {
                Outcome::Ok => {}
                Outcome::Fail(c, _) => assert!(f.contains_key(&c), "{n}: {c}"),
            }
        }
    }
}

#[test]
fn f4_effectful_spec_is_rejected_cleanly() {
    assert!(wf_error("g a:Account\n  ensure ledger.debit a 5\nbody\n  ledger.credit a 0\n").contains("specs may only read state"));
}

// ===================================================================================== F6. Why is lazy, hash-anchored

fn store(path: &str) -> why::Store {
    why::load(&format!("{ROOT}/{path}")).unwrap()
}

#[test]
fn f6_body_change_marks_why_stale() {
    let st = store("examples/ledger-tour/ledger_v4.why");
    let fresh = parse(&v4());
    assert!(!why::status(&st, &fresh.defs["transfer"], &fresh).stale);
    let defs = parse(&v4().replace("  ledger.credit to amount\n  audit", "  ledger.credit to amount\n  ledger.credit to 0\n  audit"));
    assert!(why::status(&st, &defs.defs["transfer"], &defs).stale);
}

#[test]
fn f6_eg_only_change_does_not_mark_why_stale() {
    // examples are evidence, not semantics, so why stays fresh
    let st = store("examples/ledger-tour/ledger_v4.why");
    let defs = parse(&v4().replace("transfer A B 100 -> ok | A:400 B:100", "transfer A B 7 -> ok | A:493 B:7"));
    assert!(!why::status(&st, &defs.defs["transfer"], &defs).stale);
}

#[test]
fn f6_hash_distinguishes_definitions_with_different_examples() {
    let a = parse(&v4()).defs["transfer"].clone();
    let b = parse(&v4().replace("transfer A B 100 -> ok | A:400 B:100", "transfer A B 7 -> ok | A:493 B:7")).defs["transfer"].clone();
    assert!(a.hash() == b.hash() && a.content_hash() != b.content_hash());
}

#[test]
fn f6_callee_interface_change_marks_caller_why_stale() {
    let v1 = parse(&read("examples/ledger-tour/ledger_v1.k"));
    let v2 = parse(&read("examples/ledger-tour/ledger_v2.k"));
    assert_eq!(v1.defs["payout"].hash(), v2.defs["payout"].hash());
    assert_ne!(why::anchor_hash(&v1, &v1.defs["payout"]), why::anchor_hash(&v2, &v2.defs["payout"]));
    let st = store("examples/ledger-tour/ledger.why"); // anchored to v3 (= v2 for payout)
    assert!(why::status(&st, &v1.defs["payout"], &v1).stale && !why::status(&st, &v2.defs["payout"], &v2).stale);
}

#[test]
fn f6_orphaned_why_entries_are_reported() {
    let st = store("examples/ledger-tour/ledger_v4.why");
    let defs = parse(&v4().replace("transfer", "move"));
    assert_eq!(why::orphans(&st, &defs).into_keys().collect::<Vec<_>>(), ["transfer"]);
    let path = tmp("f6orph").join("x.why");
    std::fs::write(&path, read("examples/ledger-tour/ledger_v4.why")).unwrap();
    let left = why::anchor(&path.to_string_lossy(), &defs).unwrap();
    assert_eq!(left.into_keys().collect::<Vec<_>>(), ["transfer"]);
    assert!(std::fs::read_to_string(&path).unwrap().contains("transfer@")); // kept, and reported
    let reps = analyse(&defs, false, None, None).unwrap();
    let whys: IndexMap<String, why::Status> = defs.defs.iter().map(|(n, d)| (n.clone(), why::status(&st, d, &defs))).collect();
    assert!(kern::render::terminal(&reps, &defs, &whys, &why::orphans(&st, &defs)).contains("ORPHANED why: 2 entries for `transfer`"));
}

#[test]
fn f6_placeholder_anchor_is_stale_and_anchor_keeps_it() {
    let defs = parse(&v4());
    let path = tmp("f6ph").join("x.why");
    std::fs::write(&path, "transfer@PLACEHOLDER\n  why one\n  why two\n").unwrap();
    let st = why::load(&path.to_string_lossy()).unwrap();
    assert!(why::status(&st, &defs.defs["transfer"], &defs).stale);
    why::anchor(&path.to_string_lossy(), &defs).unwrap();
    let want = format!("transfer@{}\n  why one\n  why two\n", why::anchor_hash(&defs, &defs.defs["transfer"]));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), want);
}

#[test]
fn f6_malformed_why_file_is_an_error_not_a_silent_drop() {
    for text in ["transfer\n  why\n", "  orphan entry\ntransfer@0123abcd\n  why\n"] {
        assert!(why::parse(text, "x.why").is_err(), "{text:?}");
    }
}

#[test]
fn f6_stale_message_names_the_anchors_not_build_hashes() {
    let d = tmp("f6msg");
    std::fs::write(d.join("m.k"), v4()).unwrap();
    let defs = parse(&v4());
    std::fs::write(d.join("m.why"), format!("transfer@{}\n  a reason\n", defs.defs["transfer"].hash())).unwrap(); // the build hash
    let (code, out, _) = common::kern(&["why", "m.k", "transfer"], &d.to_string_lossy());
    assert_eq!(code, 0);
    let want = format!("anchored to {}, the interface is now {}", defs.defs["transfer"].hash(), why::anchor_hash(&defs, &defs.defs["transfer"]));
    assert!(out.contains(&want), "{out}");
}

#[test]
fn f6_obligation_phrased_why_is_flagged() {
    let st = store("examples/ledger-tour/ledger_v4.why");
    let defs = parse(&v4());
    let s = why::status(&st, &defs.defs["transfer"], &defs);
    let musts: Vec<String> = s.entries.iter().filter(|e| e.contains("must")).cloned().collect();
    assert!(!musts.is_empty() && s.obligations == musts);
    assert!(why::status(&st, &defs.defs["payout"], &defs).obligations.is_empty());
}

// ===================================================================================== F7. Closed world of effects

#[test]
fn f7_body_cannot_reach_the_host_language() {
    for body in ["__import__ 1", "os.system 1", "from.__class__", "print 1"] {
        let defs = parse(&format!("f from:Account to:Account\nbody\n  {body}\n"));
        let mut w = World::new(
            IndexMap::from([("A".to_string(), 1), ("B".to_string(), 2)]),
            IndexMap::from([("A".to_string(), "u".to_string()), ("B".to_string(), "u".to_string())]),
            0,
        );
        let r = Interp::new(&defs).run("f", &[Value::Str("A".into()), Value::Str("B".into())], &mut w);
        assert!(matches!(r, Err(RunError::Kern(_))), "{body}: {r:?}");
    }
}

#[test]
fn f7_unknown_names_are_rejected_before_proving() {
    assert!(wf_error("f from:Account to:Account\nbody\n  os.system 1\n").contains("unknown name `os.system`"));
}

#[test]
fn f7_values_do_not_leak_host_semantics() {
    let e = wf_error("f a:Account b:Account\nbody\n  audit.emit (Foo (a + b))\n");
    assert!(e.contains("`+` needs Int and Int, got Account and Account"), "{e}");
}

#[test]
fn f7_type_errors_are_rejected() {
    for (body, msg) in [
        ("ledger.credit a (b - b)", "needs Int and Int, got Account and Account"),
        ("ledger.credit n a", "argument 1 of `ledger.credit` must be Account, got Int"),
        ("need a > b else Bad", "needs Int and Int, got Account and Account"),
        ("need ledger.owner a = n else Bad", "compares Owner with Int"),
        ("need n else Bad", "must be Bool, got Int"),
        ("ledger.balance a", "value of type Int is computed and discarded"),
        ("audit.emit n", "argument 1 of `audit.emit` must be Event, got Int"),
        ("x = ledger.credit a n", "has no value (Unit)"),
    ] {
        let e = wf_error(&format!("f a:Account b:Account n:Int\nbody\n  {body}\n"));
        assert!(e.contains(msg), "{body}: {e}");
    }
}

// ===================================================================================== F9. Tokens

#[test]
fn f9_keywords_are_one_token() {
    let t = kern::views::tokens;
    if t("x").is_none() {
        return eprintln!("SKIP: no tokenizer");
    }
    for word in "need ensure keep uses fails eg why else body emit".split(' ') {
        assert_eq!(t(&format!("  {word} x")).unwrap() - t("  x").unwrap(), 1, "{word}");
    }
}

#[test]
fn f9_editor_view_is_constant_as_the_module_grows() {
    let t = kern::views::tokens;
    if t("x").is_none() {
        return eprintln!("SKIP: no tokenizer");
    }
    let re = Regex::new(r"\b(transfer|payout|settle|deposit|withdraw|rebalance|month_end)\b").unwrap();
    let mut sizes = vec![];
    for k in [1, 4] {
        let src: Vec<String> = (0..k).map(|i| re.replace_all(&large(), |c: &regex::Captures| format!("{}{}", &c[1], i)).into_owned()).collect();
        let src = src.join("\n\n");
        let (defs, reps) = analysed(&src, false);
        sizes.push((t(&src).unwrap(), t(&kern::views::editor(&reps, "month_end0", &no_whys(&defs))).unwrap()));
    }
    let ((m1, e1), (m4, e4)) = (sizes[0], sizes[1]);
    assert!(e1 == e4 && m4 as f64 > 3.9 * m1 as f64);
}

#[test]
fn f9_editor_view_beats_reading_the_call_chain_only_from_depth_two() {
    let t = kern::views::tokens;
    if t("x").is_none() {
        return eprintln!("SKIP: no tokenizer");
    }
    let src = large();
    let (defs, reps) = analysed(&src, false);
    // each definition's source block: from its header to the next column-0 definition header
    let heads: Vec<(usize, String)> = Regex::new(r"(?m)^([a-z]\w*) ").unwrap().captures_iter(&src).map(|c| (c.get(0).unwrap().start(), c[1].to_string())).collect();
    let mut blocks: IndexMap<String, String> = IndexMap::new();
    for (i, (start, name)) in heads.iter().enumerate() {
        let end = heads.get(i + 1).map_or(src.len(), |h| h.0);
        blocks.insert(name.clone(), src[*start..end].trim().to_string());
    }
    fn chain(defs: &Module, n: &str, acc: &mut Vec<String>) {
        if !acc.iter().any(|x| x == n) {
            acc.push(n.to_string());
            for c in callees(defs, &defs.defs[n]) {
                chain(defs, &c, acc);
            }
        }
    }
    let ratio = |n: &str| {
        let mut acc = vec![];
        chain(&defs, n, &mut acc);
        let read: Vec<&str> = acc.iter().map(|x| blocks[x].as_str()).collect();
        t(&kern::views::editor(&reps, n, &no_whys(&defs))).unwrap() as f64 / t(&read.join("\n\n")).unwrap() as f64
    };
    assert!(ratio("month_end") < 0.45 && ratio("payout") < 0.8);
    assert!(ratio("transfer") > 1.0); // leaf: inferred lines are added and nothing is skipped
}

// ===================================================================================== incidental

#[test]
fn incidental_and_after_comparison_parses() {
    let d = parse("f a:Account amount:Int\n  need amount > 0 and amount < 10 else Bad\nbody\n  ledger.credit a amount\n").defs["f"].clone();
    let Need::Need(c, _, _) = &d.needs[0] else { panic!() };
    assert_eq!(*c, Expr::Bin("and", Box::new(parse_expr("amount > 0", None).unwrap()), Box::new(parse_expr("amount < 10", None).unwrap())));
}

#[test]
fn incidental_boolean_clauses_print_and_reparse() {
    for src in ["a > 0 and b < 10", "a > 0 or b = c and d != e", "(a or b) and c", "a and (b or c)", "a + 1 > b - 2 and c"] {
        let e = parse_expr(src, None).unwrap();
        assert_eq!(parse_expr(&code(&e), None).unwrap(), e, "{src}");
    }
}

#[test]
fn incidental_why_for_unknown_name_is_a_clean_error() {
    let (code, _, err) = common::kern(&["why", "examples/ledger-tour/ledger_v4.k", "nosuch", "--why", "examples/ledger-tour/ledger_v4.why"], ROOT);
    assert_eq!(code, 2);
    assert!(err.contains("no definition named `nosuch`"), "{err}");
}

// ===================================================================================== parse and type errors (S1-S4)

#[test]
fn s1_branch_scoping_and_types() {
    for (body, msg) in [
        ("  if amount > 0\n    x = amount\n  else\n    ledger.credit to x", "unknown name `x`"),
        ("  if amount > 0\n    x = amount\n  else\n    x = 1", "shadows"),
        ("  if amount\n    ledger.credit to 1", "must be Bool"),
    ] {
        let e = wf_error(&format!("f to:Account amount:Int\nbody\n{body}\n"));
        assert!(e.contains(msg), "{body}: {e}");
    }
}

#[test]
fn s1_when_is_only_a_clause_ending() {
    for (spec, msg) in [
        ("  need amount > 0 when amount > 1 else Bad", "`when` is only allowed"),
        ("  ensure ledger.balance' to = 1 when ledger.balance' to = 1", "only allowed in `ensure`"),
        ("  emit Ev to when ledger.balance' to > 0", "only allowed in `ensure`"),
    ] {
        let e = wf_error(&format!("f to:Account amount:Int\n{spec}\nbody\n  ledger.credit to amount\n"));
        assert!(e.contains(msg), "{spec}: {e}");
    }
}

#[test]
fn s1_bad_blocks_are_parse_errors() {
    for src in ["f a:Int\nbody\n  else\n", "f a:Int\nbody\n  if a > 0\n  ledger.total\n", "f a:Int\nbody\n  ledger.total\n    ledger.total\n"] {
        assert!(parse_module(src, true).is_err(), "{src:?}");
    }
}

#[test]
fn s1_and_binds_tighter_than_or() {
    assert_eq!(code(&parse_expr("a or b and c", None).unwrap()), "a or b and c");
    assert!(matches!(parse_expr("a or b and c", None).unwrap(), Expr::Bin("or", _, _)));
    assert_eq!(code(&parse_expr("(a or b) and c", None).unwrap()), "(a or b) and c");
}

#[test]
fn s2_only_linear_arithmetic() {
    for (expr, msg) in [("amount / 0", "positive number"), ("amount / amount", "positive number"), ("amount * amount", "number on one side")] {
        let e = wf_error(&format!("f to:Account amount:Int\nbody\n  ledger.credit to ({expr})\n"));
        assert!(e.contains(msg), "{expr}: {e}");
    }
}

#[test]
fn s2_bad_references_are_parse_errors() {
    for (src, msg) in [
        ("f a:Int\n  need as nothing a\nbody\n  ledger.total\n", "no definition named"),
        ("f a:Int\n  need as g a\nbody\n  ledger.total\n\ng a:Int\n  need as f a\nbody\n  ledger.total\n", "loop"),
        ("f a:Int\n  need as g a a\nbody\n  ledger.total\n\ng a:Int\nbody\n  ledger.total\n", "takes 1"),
    ] {
        let e = parse_module(src, true).unwrap_err().0;
        assert!(e.contains(msg), "{e}");
    }
}

#[test]
fn s2_state_ops_are_typed() {
    for (body, msg) in [("  ledger.credit to (hold.get to)", "must be Int, got Account"), ("  hold.set id", "takes 2")] {
        let e = wf_error(&format!("{STATE_HEAD}f to:Account id:Int\nbody\n{body}\n"));
        assert!(e.contains(msg), "{body}: {e}");
    }
}

#[test]
fn s2_specs_may_read_state_but_not_write_it() {
    assert!(wf_error(&format!("{STATE_HEAD}f id:Int\n  ensure hold.set id 1\nbody\n  hold.set id 1\n")).contains("has an effect"));
}

#[test]
fn s3_functions_are_pure_total_and_only_they_appear_in_specs() {
    for (src, msg) in [
        ("f a:Int -> Int\nbody\n  need a > 0 else Bad\n  result = a\n", "total"),
        ("f a:Account -> Int\nbody\n  result = ledger.balance a\n", "pure"),
        ("f a:Int -> Int\nbody\n  if a > 0\n    result = a\n", "every path"),
        ("g a:Account\n  ensure ledger.balance' a = 0\nbody\n  h a\n\nh a:Account\nbody\n  ledger.credit a 0\n\nk a:Account\n  ensure h a\nbody\n  h a\n", "only functions"),
    ] {
        let e = wf_error(src);
        assert!(e.contains(msg), "{src:?}: {e}");
    }
}

#[test]
fn s4_loop_forms_are_checked() {
    for (src, msg) in [
        ("f lo:Int hi:Int\nbody\n  for k from lo to hi\n    for m from lo to hi\n      ledger.total\n", "nested loops"),
        ("f a:Account\n  need a != a for j from 0 to 1 else Bad\nbody\n  ledger.credit a 1\n", "only allowed at the end"),
        ("f a:Account\nbody\n  invariant ledger.total' = ledger.total\n  ledger.credit a 1\n", "start of a `for` loop"),
    ] {
        let e = wf_error(src);
        assert!(e.contains(msg), "{src:?}: {e}");
    }
}

// ===================================================================================== S1: the ratchet sees refusals

fn against_baseline(before: &str, after: &str) -> IndexMap<String, Report> {
    let (bdefs, breps) = analysed(before, true);
    let base = baseline::parse(&baseline::text(&breps, &bdefs));
    let defs = parse(after);
    analyse(&defs, false, Some(&base), None).unwrap()
}

#[test]
fn s1_dropped_need_is_flagged() {
    // C1's B10: refund drops `part <= amount`; nothing else notices, the ratchet now does
    let e = escrow1();
    let reps = against_baseline(&e, &e.replace("need part > 0 and part <= amount else BadPart", "need part > 0 else BadPart"));
    let msg = reps["refund"].regression.join(" ");
    assert!(msg.contains("refusals removed or changed") && msg.contains("part <= amount"), "{msg}");
    assert_eq!(kern::render::status(&reps["refund"]), ("warn", "Spec changed since baseline".to_string()));
}

#[test]
fn s1_dodging_need_is_flagged_without_ok_otherwise_or_eg() {
    let before = escrow1().replace("  ok otherwise\n", "");
    let after = before
        .replace(
            "capture escrow:Account seller:Account house:Account amount:Int fee:Int\n",
            "capture escrow:Account seller:Account house:Account amount:Int fee:Int\n  need amount >= 100 or amount < 50 else Unsupported\n",
        )
        .replace("  eg E:100@u0 B:0@u2 H:0@u9 | capture E B H 50 5 -> ok | E:50 B:50 H:0\n", "");
    let reps = against_baseline(&before, &after);
    assert!(reps["capture"].regression.iter().any(|x| x.contains("new refusals") && x.contains("Unsupported")));
}

#[test]
fn s1_removed_or_rewritten_guarantees_are_flagged() {
    let e = escrow1();
    for (old, new) in [
        ("  ok otherwise\n  keep ledger.total\n  ensure ledger.balance' from = ledger.balance from - amount\n  ensure ledger.balance' to = ledger.balance to + amount\n  emit Moved from to amount clock.now\n  eg A:500@u1 B:0@u2 | transfer",
         "  keep ledger.total\n  ensure ledger.balance' from = ledger.balance from - amount\n  ensure ledger.balance' to = ledger.balance to + amount\n  emit Moved from to amount clock.now\n  eg A:500@u1 B:0@u2 | transfer"),
        ("ensure ledger.balance' a = ledger.balance a + part\n", "ensure ledger.balance' a >= ledger.balance a\n"),
    ] {
        assert!(e.contains(old));
        let reps = against_baseline(&e, &e.replacen(old, new, 1));
        assert!(reps.values().any(|r| !r.regression.is_empty()));
    }
}

#[test]
fn s1_unchanged_and_strengthened_specs_are_not_flagged() {
    let e = escrow1();
    assert!(against_baseline(&e, &e).values().all(|r| r.regression.is_empty()));
    let stronger = e.replacen(
        "  ensure ledger.balance' seller = ledger.balance seller + amount\n",
        "  ensure ledger.balance' seller = ledger.balance seller + amount\n  ensure ledger.balance' buyer = ledger.balance buyer - amount\n",
        1,
    );
    assert_ne!(stronger, e);
    assert!(against_baseline(&e, &stronger)["tip"].regression.is_empty());
}

#[test]
fn s1_old_format_baseline_does_not_flag_every_need() {
    let (_, reps) = analysed(&escrow1(), true);
    let old: String = reps
        .iter()
        .map(|(n, r)| {
            let m = r.mutation.as_ref().unwrap();
            format!("{n} {}/{}\n", m.guarantee_killed, m.total) + &baseline::clauses(r).iter().map(|c| format!("  {c}\n")).collect::<String>()
        })
        .collect();
    let defs = parse(&escrow1());
    let again = analyse(&defs, false, Some(&baseline::parse(&old)), None).unwrap();
    assert!(again.values().all(|r| r.regression.is_empty()));
}

#[test]
fn s1_against_a_committed_baseline_ignores_the_file() {
    // `check --against REV` reads the baseline from git, so rewriting FILE.strength changes nothing. Run from the
    // root and from a subdirectory. Skipped without git.
    if std::process::Command::new("git").arg("--version").output().map_or(true, |o| !o.status.success()) {
        return eprintln!("SKIP: git is not usable here");
    }
    let d = tmp("s1against");
    let git = |args: &[&str]| {
        let o = std::process::Command::new("git").args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).current_dir(&d).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };
    let dir = d.to_string_lossy().into_owned();
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("sub/m.k"), escrow1()).unwrap();
    git(&["init", "-q"]);
    assert_eq!(common::kern(&["baseline", "sub/m.k"], &dir).0, 0);
    git(&["add", "sub"]);
    git(&["commit", "-qm", "base"]);
    let weak = escrow1().replacen("  need from != to else SameAccount\n", "", 1);
    assert_ne!(weak, escrow1());
    std::fs::write(d.join("sub/m.k"), &weak).unwrap();
    assert_eq!(common::kern(&["baseline", "sub/m.k"], &dir).0, 0); // the edit rewrites the baseline file
    let flag = "need from != to else SameAccount";
    assert!(!common::kern(&["check", "sub/m.k", "--quick"], &dir).1.contains(flag)); // the rewritten file hides it
    for (cwd, path) in [(dir.clone(), "sub/m.k"), (format!("{dir}/sub"), "m.k")] {
        let (code, out, err) = common::kern(&["check", path, "--quick", "--against", "HEAD"], &cwd);
        assert!(code == 1 && out.contains("SPEC CHANGED SINCE BASELINE") && out.contains(flag), "{out}{err}");
    }
}

// ===================================================================================== S2: incremental, quick check

fn cached(reps: &IndexMap<String, Report>) -> BTreeSet<String> {
    reps.iter().filter(|(_, r)| r.cached).map(|(n, _)| n.clone()).collect()
}

#[test]
fn s2_only_the_changed_leaf_and_its_callers_are_rechecked() {
    let cache = tmp("s2leaf").to_string_lossy().into_owned();
    let src = common::chains(3, 1, 2, 30, false); // 10 chains f{c}_1 <- f{c}_2 <- f{c}_3
    analyse(&parse(&src), false, None, Some(&cache)).unwrap();
    let changed = src.replacen("| f4_1 A B 100 -> ok", "| f4_1 A B 90 -> ok", 1).replace(
        "A:500@u1 B:0@u2 | f4_1 A B 90 -> ok | A:400 B:100",
        "A:500@u1 B:0@u2 | f4_1 A B 90 -> ok | A:410 B:90",
    );
    let reps = analyse(&parse(&changed), false, None, Some(&cache)).unwrap();
    let rechecked: BTreeSet<String> = reps.keys().filter(|n| !cached(&reps).contains(*n)).cloned().collect();
    assert_eq!(rechecked, ["f4_1", "f4_2", "f4_3"].iter().map(|s| s.to_string()).collect()); // the leaf and its callers
    assert!(reps.values().all(|r| r.clean));
}

#[test]
fn s2_a_callee_body_change_is_never_served_from_the_cache() {
    // callers depend on what is inferred from callee bodies (writes, lifted refusals)
    let cache = tmp("s2callee").to_string_lossy().into_owned();
    analyse(&parse(&escrow1()), false, None, Some(&cache)).unwrap();
    let src = escrow1().replacen(
        "  ledger.debit from amount\n  ledger.credit to amount\n",
        "  ledger.debit from amount\n  ledger.credit to amount\n  ledger.credit from 0\n",
        1,
    );
    let c = cached(&analyse(&parse(&src), false, None, Some(&cache)).unwrap());
    assert!(!c.contains("transfer") && !c.contains("move") && !c.contains("purchase"));
}

#[test]
fn s2_a_corrupt_cache_entry_is_recomputed_not_trusted() {
    let dir = tmp("s2corrupt");
    let cache = dir.to_string_lossy().into_owned();
    analyse(&parse(&escrow1()), false, None, Some(&cache)).unwrap();
    fn corrupt(p: &std::path::Path) {
        for e in std::fs::read_dir(p).unwrap().flatten() {
            if e.path().is_dir() {
                corrupt(&e.path());
            } else {
                std::fs::write(e.path(), b"not json").unwrap();
            }
        }
    }
    corrupt(&dir);
    let reps = analyse(&parse(&escrow1()), false, None, Some(&cache)).unwrap();
    assert!(cached(&reps).is_empty() && reps.values().all(|r| r.clean));
}

#[test]
fn s2_quick_never_shows_an_unmeasured_strength() {
    let cache = tmp("s2quick").to_string_lossy().into_owned();
    let term = |src: &str, mutation: bool| {
        let defs = parse(src);
        let reps = analyse(&defs, mutation, None, Some(&cache)).unwrap();
        kern::render::terminal(&reps, &defs, &no_whys(&defs), &IndexMap::new())
    };
    let out = term(&escrow1(), false);
    assert!(out.contains("spec strength: not measured (--quick)") && !out.contains("spec strength: guarantees"));
    term(&escrow1(), true); // a full run measures it
    assert!(term(&escrow1(), false).contains("spec strength: guarantees 10/10")); // same code: current, shown
    let src = escrow1().replace("  eg A:500@u1 B:0@u2 | transfer A B 0 -> BadAmount\n", "");
    assert!(term(&src, false).split("\n\n").next().unwrap().contains("not measured")); // changed: not shown
}

// ===================================================================================== S5. Named constants (F4, F7)
// `const NAME N` (DECISIONS.md 2026-11-05, lang) is substituted at load time, so a constant must mean exactly its
// literal everywhere: same AST, same hash, so run, infer, prove, mutants, host and emitter cannot disagree.

/// Every single-file example, package and probe (examples/apps/ is app's work in progress, not a fixture).
fn corpus() -> Vec<(String, Vec<String>)> {
    let mut out = vec![];
    let mut ks = |dir: &std::path::Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "k")).map(|p| p.to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    let mut dirs: Vec<_> = std::fs::read_dir(format!("{ROOT}/examples")).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.is_dir() && !p.ends_with("apps")).collect();
    dirs.sort();
    for d in dirs {
        if d.join("kern.pkg").exists() {
            let mut files = vec![];
            fn walk(d: &std::path::Path, files: &mut Vec<String>) {
                for e in std::fs::read_dir(d).unwrap().flatten() {
                    let p = e.path();
                    if p.is_dir() && !p.ends_with("proof-pages") {
                        walk(&p, files);
                    } else if p.extension().is_some_and(|x| x == "k") {
                        files.push(p.to_string_lossy().into_owned());
                    }
                }
            }
            walk(&d, &mut files);
            files.sort();
            out.push((d.to_string_lossy().into_owned(), files));
        } else {
            out.extend(ks(&d).into_iter().map(|f| (f.clone(), vec![f])));
        }
    }
    out.extend(ks(std::path::Path::new(&format!("{ROOT}/tests/probes"))).into_iter().map(|f| (f.clone(), vec![f])));
    out
}

#[test]
fn s5_every_literal_hoisted_to_a_constant_means_the_same_program() {
    // F4/F7: across the whole corpus, writing a literal as a named constant changes nothing the checker sees
    let mut hoisted = 0;
    let corpus = corpus();
    assert!(corpus.len() >= 20, "{}", corpus.len());
    for (path, files) in &corpus {
        let plain = kern::package::load(path, &IndexMap::new()).unwrap_or_else(|e| panic!("{path}: {}", e.0));
        let mut over = IndexMap::new();
        for (i, f) in files.iter().enumerate() {
            let (text, n) = common::hoist_literals(&std::fs::read_to_string(f).unwrap(), &i.to_string());
            hoisted += n;
            over.insert(f.clone(), text);
        }
        let consts = kern::package::load(path, &over).unwrap_or_else(|e| panic!("{path} hoisted: {}", e.0));
        common::assert_same_meaning(&plain, &consts, path);
    }
    assert!(hoisted > 300, "only {hoisted} literals hoisted");
}

#[test]
fn s5_a_constant_means_its_value_in_every_position() {
    // the probe uses constants in needs, failure payloads, ensures, emits, `need as`/`emit as`, loop bounds and
    // invariants, `if`, `let`, `*` and `/`; with each written as its value it is the same program
    let src = read("tests/probes/consts.k");
    let lit = common::inline_consts(&src);
    assert!(!lit.contains("fee") && lit.contains("TooSmall 5"));
    let (a, b) = (parse(&src), parse(&lit));
    kern::wf::check_module(&a).unwrap();
    common::assert_same_meaning(&a, &b, "consts.k");
    let (ra, rb) = (analyse(&a, false, None, None).unwrap(), analyse(&b, false, None, None).unwrap());
    let show = |defs: &Module, r: &IndexMap<String, Report>| kern::render::terminal(r, defs, &no_whys(defs), &IndexMap::new());
    assert_eq!(show(&a, &ra), show(&b, &rb));
    assert!(ra.values().all(|r| r.clean), "{}", show(&a, &ra));
}

#[test]
fn s5_changing_a_constant_stales_exactly_the_definitions_that_use_it() {
    // F6 (design rule 4): a why entry must not silently describe the old value
    let src = read("tests/probes/consts.k");
    let base = parse(&src);
    let anchors = |m: &Module| -> IndexMap<String, String> {
        m.defs.iter().map(|(n, d)| (n.clone(), why::anchor_hash(m, d))).collect()
    };
    let before = anchors(&base);
    for (from, to, users) in [
        ("const fee 5", "const fee 6", vec!["charge", "relay", "scaled"]),
        ("const lim 100", "const lim 101", vec!["charge", "relay", "bump"]),
        ("const cap 3", "const cap 4", vec!["bump"]),
        ("const two 2", "const two 3", vec!["half"]),
        ("const unused 7", "const unused 8", vec![]),
    ] {
        let after = anchors(&parse(&src.replace(from, to)));
        let changed: Vec<&str> = before.keys().filter(|n| before[*n] != after[*n]).map(|s| s.as_str()).collect();
        assert_eq!(changed, users, "{from} -> {to}");
    }
}

#[test]
fn s5_constant_errors_are_reported_not_shadowed() {
    // DECISIONS.md 2026-11-05 (lang), "Errors, not shadowing"; each must be an error before anything runs
    let head = "const fee 5\nstate hold Int\n\n";
    let cases: &[(&str, &str)] = &[
        ("f fee:Int\n  ok otherwise\nbody\n  hold.set 1 fee\n", "parameter fee has the name of a constant"),
        ("f k:Int\n  ok otherwise\nbody\n  fee = 3\n  hold.set k fee\n", "`let` fee has the name of a constant"),
        ("f lo:Int hi:Int\n  ok otherwise\nbody\n  for fee from lo to hi\n    hold.set fee 1\n", "loop variable fee"),
        ("f lo:Int hi:Int\n  ok otherwise\n  ensure hold.get' fee = 0 for fee from lo to hi\nbody\n  hold.set lo 0\n",
         "range variable fee"),
        ("invariant fee:Int hold.get fee >= 0\n", "variable fee has the name of a constant"),
        ("f k:Int\n  ok otherwise\n  ensure hold.get' k = fee'\nbody\n  hold.set k fee\n", "a constant has no after-state"),
        ("f k:Int\n  ok otherwise\nbody\n  hold.set k (fee k)\n", "constant fee takes no arguments"),
        ("fee k:Int\n  ok otherwise\nbody\n  hold.set k 1\n", "constant fee has the name of a definition"),
        ("const fee 6\n", "constant fee is declared twice"),
        ("f x:Int -> Int\n  ensure result = x\nbody\n  result = x / zero\nconst zero 0\n", "needs a positive number"),
    ];
    for (body, want) in cases {
        let e = wf_error(&format!("{head}{body}"));
        assert!(e.contains(want), "{body:?}: want {want:?}, got {e:?}");
    }
    for bad in ["const fee -5", "const fee 2 + 3", "const fee other", "const Fee 5", "const fee", "const fee 5 6",
                "const need 5", "const result 5"] {
        assert!(!wf_error(&format!("{bad}\nstate hold Int\n")).is_empty(), "{bad} was accepted");
    }
    let e = wf_error("const hold 5\nstate hold Int\n");
    assert!(e.contains("name of a state"), "{e}");
    let e = wf_error("const console 5\nchannel console\n");
    assert!(e.contains("name of a channel"), "{e}");
}
