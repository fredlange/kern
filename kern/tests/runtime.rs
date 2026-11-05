//! Rust runtime tests (port of tests/test_runtime.py; docs/research/RUNTIME_PLAN.md): the compiled program against
//! the interpreter. Skipped without cargo. (The random differential run, validation/difftest.py, ports with the
//! validation tools.)

mod common;
use common::{tmp, ROOT};
use indexmap::IndexMap;
use kern::run::{Interp, Outcome, Value, World};
use std::sync::OnceLock;

const MAX: i128 = i64::MAX as i128;

fn have_cargo() -> bool {
    std::process::Command::new("cargo").arg("--version").output().is_ok()
}

fn build(path: &str) -> String {
    let crate_dir = kern::emit_rust::emit(path, None).unwrap();
    kern::emit_rust::build(&crate_dir).unwrap()
}

fn escrow() -> &'static str {
    static B: OnceLock<String> = OnceLock::new();
    B.get_or_init(|| build(&format!("{ROOT}/examples/escrow/escrow_s2.k")))
}

fn probes() -> &'static str {
    static B: OnceLock<String> = OnceLock::new();
    B.get_or_init(|| build(&format!("{ROOT}/tests/probes/runtime.k")))
}

fn run(binary: &str, lines: &[&str]) -> Vec<String> {
    use std::io::Write;
    let mut c = std::process::Command::new(binary)
        .arg("run")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(c.stdin.take().unwrap(), "{}", lines.join("\n")).unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success());
    String::from_utf8(o.stdout).unwrap().lines().map(String::from).collect()
}

// ---- C3: Int is i64 at run time; leaving it is a fault: atomic, no clock tick, not a refusal

#[test]
fn overflow_after_a_write_rolls_back_and_does_not_tick() {
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    // transfer debits `from`, then the credit to `to` overflows: the debit is undone too
    let world = format!("world A:100@u1 B:{}@u2 now:7", MAX - 5);
    let out = run(escrow(), &[&world, "dump", "call transfer A B 10", "dump"]);
    assert_eq!(out[1], "fault t=7");
    let want = format!("clock=7 | A:100@u1 B:{}@u2 |", MAX - 5);
    assert_eq!((out[0].as_str(), out[2].as_str()), (want.as_str(), want.as_str()));
}

#[test]
fn overflow_is_a_deliberate_divergence_from_the_interpreter() {
    // the interpreter's Int is i128 (decision O1), so the same call succeeds there
    let defs = kern::package::load(&format!("{ROOT}/examples/escrow/escrow_s2.k"), &IndexMap::new()).unwrap();
    let mut w = World::new(
        IndexMap::from([("A".to_string(), 100), ("B".to_string(), MAX - 5)]),
        IndexMap::from([("A".to_string(), "u1".to_string()), ("B".to_string(), "u2".to_string())]),
        7,
    );
    let args = [Value::Str("A".into()), Value::Str("B".into()), Value::Int(10)];
    assert_eq!(Interp::new(&defs).run("transfer", &args, &mut w).unwrap(), Outcome::Ok);
    assert_eq!(w.bal["B"], MAX + 5);
}

#[test]
fn overflow_in_an_expression_is_a_fault() {
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    let out = run(probes(), &[&format!("world A:{MAX}@u1 B:1@u2"), "call snapshot 0", "dump"]);
    assert_eq!(out[0], "fault t=0");
    assert_eq!(out[1], format!("clock=0 | A:{MAX}@u1 B:1@u2 |"));
}

#[test]
fn a_refusal_still_ticks_and_a_bad_call_does_not() {
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    let out = run(escrow(), &["world A:5@u1 B:0@u2 now:3", "call transfer A B 10", "call transfer A Z 1", "call transfer A B x"]);
    assert_eq!(out, ["refused Insufficient 5 t=4", "badcall t=4", "badcall t=4"]);
}

#[test]
fn floor_division_matches_the_interpreter() {
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    // (n - 7) / 2 for n < 7 rounds toward minus infinity, as in Z3 and the interpreter
    let out = run(probes(), &["world A:0@u1", "call use_half A 0", "dump"]);
    assert_eq!(out[0], "ok t=1");
    assert!(out[1].contains("mark.A:-4"), "{}", out[1]); // (0 - 7) / 2 = -3.5 -> -4, not -3
}

#[test]
fn unsupported_is_loud_never_wrong() {
    // a construct the generator can't compile is refused; it never emits something else
    let d = tmp("rtnested");
    let p = d.join("nested.k");
    std::fs::write(&p, "wrap a:Account\nbody\n  audit.emit (Outer (Inner a))\n").unwrap();
    let e = kern::emit_rust::emit(&p.to_string_lossy(), Some(&d.join("out").to_string_lossy())).unwrap_err();
    assert!(matches!(e, kern::emit_rust::BuildError::Unsupported(_)), "{e}");
}

#[test]
fn compiled_sums_give_the_hand_computed_values() {
    // c4eb9a1 (#2W5YSA6QFQ), runtime's probe with its by-hand values: a sum in a need, in a function, in an event,
    // a filtered sum reading the clock, `of k * 2 where k > lo`, and a reversed range (0)
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    let src = "state score Int\nstate due Int\nstate flag Account\n\n\
total lo:Int hi:Int -> Int\nbody\n  result = sum k from lo to hi of score.get k\n\n\
overdue lo:Int hi:Int -> Int\nbody\n  result = sum k from lo to hi of 1 where due.get k < clock.now and score.get k > 0\n\n\
record_total lo:Int hi:Int\n  need hi - lo <= 20 else TooWide\n  need (sum k from lo to hi of score.get k) >= 0 else Negative\n  ok otherwise\nbody\n  score.set 0 (total lo hi)\n  audit.emit (Counted (overdue lo hi) (sum k from lo to hi of k * 2 where k > lo))\n\n\
bump a:Account n:Int\n  need n > 0 else Bad\nbody\n  flag.set a (sum j from 0 to n of j)\n  score.set n (sum j from n to 0 of 5)\n";
    let p = tmp("compiled-sums").join("sums_rt.k");
    std::fs::write(&p, src).unwrap();
    let binary = build(p.to_str().unwrap());
    let world = "world A:0@u1 score.1:3 score.2:4 due.1:1 due.2:9 now:5";
    let out = run(&binary, &[world, "call record_total 1 3", "call total 1 3", "call overdue 1 3", "call bump A 4",
                              "call record_total 0 30", "dump"]);
    assert!(out[0].starts_with("ok") && out[0].contains("Counted 1 4"), "{out:?}");
    assert!(out[1].starts_with("ok result=7"), "{out:?}");
    assert!(out[2].starts_with("ok result=1"), "{out:?}");
    assert!(out[3].starts_with("ok"), "{out:?}");
    assert!(out[4].starts_with("refused TooWide"), "{out:?}");
    assert!(out[5].contains("flag.A:6") && out[5].contains("score.0:7"), "{out:?}");
    // the interpreter agrees on every line
    let defs = kern::package::load(p.to_str().unwrap(), &IndexMap::new()).unwrap();
    let r = interpreter_total_and_overdue(&defs);
    assert_eq!(r, (7, 1));
}

/// total 1 3 and overdue 1 3 in the interpreter, on the same world.
fn interpreter_total_and_overdue(defs: &kern::syntax::Module) -> (i128, i128) {
    let (rest, maps) = kern::syntax::split_maps("A:0@u1 score.1:3 score.2:4 due.1:1 due.2:9").unwrap();
    let mut w = World::from_example(&kern::syntax::parse_world(&rest, None).unwrap(), 5, &maps);
    let mut it = Interp::new(defs);
    let mut get = |name: &str| {
        assert!(matches!(it.run(name, &[Value::Int(1), Value::Int(3)], &mut w).unwrap(), Outcome::Ok));
        it.last_result.clone().unwrap().num().unwrap()
    };
    (get("total"), get("overdue"))
}
