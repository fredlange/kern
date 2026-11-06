//! The random differential half of tests/test_runtime.py: every example and the probes, the interpreter against
//! the compiled binary, and the packages. Skipped without cargo.

use indexmap::IndexMap;
use kern::pyrandom::Random;
use kern_validation::difftest::{build_case, example_modules, run_module, ROOT};

fn have_cargo() -> bool {
    std::process::Command::new("cargo").arg("--version").output().is_ok()
}

#[test]
fn differential_small() {
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    let mut mods = example_modules();
    mods.push(format!("{ROOT}/tests/probes/runtime.k"));
    mods.push(format!("{ROOT}/tests/probes/invcheck.k"));
    mods.push(format!("{ROOT}/tests/probes/sums.k"));
    mods.push(format!("{ROOT}/tests/probes/texts.k"));
    mods.push(format!("{ROOT}/tests/probes/decimals.k"));
    mods.push(format!("{ROOT}/tests/probes/remainder.k"));
    for m in mods {
        let r = run_module(&m, 100, 1, 5).unwrap_or_else(|e| panic!("{m}: {e}"));
        assert!(r.eg_failures.is_empty() && r.n_divergences == 0, "{m}: {r:?}");
    }
}

#[test]
fn a_package_compiles_and_matches_the_interpreter() {
    // `kern build` takes packages: every eg line and random calls give the same results as the interpreter
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    for app in ["shop", "library", "lottery"] {
        let r = run_module(&format!("{ROOT}/examples/{app}"), 100, 1, 5).unwrap();
        assert!(r.egs > 0 && r.eg_failures.is_empty() && r.n_divergences == 0, "{app}: {r:?}");
    }
}

#[test]
fn compiled_invariant_checks_are_exercised_and_a_binary_without_them_is_caught() {
    // f53e1a0: a module invariant a definition doesn't prove is checked after its compiled calls. The reference
    // (interpreter + the host's check at written keys) must produce such faults on tests/probes/invcheck.k, and a
    // binary built without the checks must diverge on exactly those lines.
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    let path = format!("{ROOT}/tests/probes/invcheck.k");
    let defs = kern::package::load(&path, &IndexMap::new()).unwrap();
    let checks = kern::emit_rust::unproven_invariants(&defs).unwrap();
    let mut names: Vec<&str> = checks.keys().map(|s| s.as_str()).collect();
    names.sort();
    // restore (writes the value back) is proven since 4b44b2f, so it gets no check
    assert_eq!(names, ["late", "mark", "pay", "put", "relay", "sweep"]);
    let (with, _) = build_case(&defs, &checks, &mut Random::from_int(1), 2000, 1, None).unwrap();
    let (without, _) = build_case(&defs, &IndexMap::new(), &mut Random::from_int(1), 2000, 1, None).unwrap();
    assert_eq!(with.script, without.script);
    // calls that succeed in the interpreter and become faults under the check (later lines then differ in clock and
    // world, which the comparison below covers)
    let first: Vec<usize> = (0..with.expected.len())
        .filter(|&i| with.expected[i].starts_with("fault t=") && without.expected[i].starts_with("ok"))
        .collect();
    assert!(first.len() > 50, "only {} calls became faults", first.len());
    // each kind of write the probe has must produce at least one fault
    for name in ["put", "relay", "sweep", "mark", "pay"] {
        assert!(first.iter().any(|&i| with.place[i].contains(&format!("| {name} "))), "no fault from {name}");
    }
    assert!(!first.iter().any(|&i| with.place[i].contains("| late ")), "late refuses: nothing to check");

    // a binary without the checks: same script, diverges where the reference faults
    let dir = format!("{}/invcheck_nocheck", kern::emit_rust::build_dir());
    std::fs::create_dir_all(format!("{dir}/src")).unwrap();
    let runtime = kern::emit_rust::runtime_dir().unwrap();
    for (rel, text) in kern::emit_rust::crate_files(&defs, "invcheck_nocheck", &runtime, IndexMap::new()).unwrap() {
        std::fs::write(format!("{dir}/{rel}"), text).unwrap();
    }
    let binary = kern::emit_rust::build(&dir).unwrap();
    let script = std::env::temp_dir().join(format!("kern-invcheck-{}.script", std::process::id()));
    std::fs::write(&script, with.script.join("\n") + "\n").unwrap();
    let out = std::process::Command::new(&binary).arg("run").arg(&script).output().unwrap();
    let _ = std::fs::remove_file(&script);
    let got: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect();
    assert_eq!(got, without.expected, "the unchecked binary should match the unchecked reference");
    assert_ne!(got, with.expected);
}

#[test]
fn names_that_meet_in_rust_compile_apart() {
    // found 2026-11-05 (#AGIZXR4NG6, #V7NI3YJFGQ): `p.x` and `p_x`, a reserved `w` and a `let w_`, `tx` and `tx_`, a
    // definition named like a generated helper, a callee writing at a record-field key. Each must build and agree
    // with the interpreter (the shadowing ones silently broke a proven guarantee before 381bfcb/804f7a7).
    if !have_cargo() {
        return eprintln!("SKIP: needs cargo");
    }
    let dir = std::env::temp_dir().join(format!("kern-names-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, src) in [
        ("record_let_shadows_field", "state hold Int\n\nrecord Point x:Int y:Int\n\nshadow p:Point\n  ok otherwise\n  ensure hold.get' 1 = p.x\n  eg | shadow 5 6 -> ok | hold.1:5 hold.3:99\nbody\n  p_x = 99\n  hold.set 1 p.x\n  hold.set 3 p_x\n"),
        ("record_param_collides_with_field", "state hold Int\n\nrecord Point x:Int y:Int\n\nshift p:Point p_x:Int\n  ok otherwise\n  ensure hold.get' 1 = p.x\n  ensure hold.get' 2 = p_x\nbody\n  hold.set 1 p.x\n  hold.set 2 p_x\n\nshadow p:Point\n  ok otherwise\n  ensure hold.get' 1 = p.x\nbody\n  p_x = 99\n  hold.set 1 p.x\n  hold.set 3 p_x\n"),
        ("record_callee_key_crash", "state hold Int\n\nrecord Point x:Int y:Int\n\nput p:Point\n  ok otherwise\n  ensure hold.get' p.x = p.y\nbody\n  hold.set p.x p.y\n\nc a:Int b:Int\n  ok otherwise\nbody\n  put (Point a b)\n"),
        ("ident_reserved_let_shadows", "state hold Int\n\ng k:Int\n  ok otherwise\n  ensure hold.get' 1 = k\nbody\n  w_ = 7\n  hold.set 1 k\n  hold.set 2 w_\n\nf w:Int\n  ok otherwise\n  ensure hold.get' 1 = w\nbody\n  w_ = 99\n  hold.set 1 w\n  hold.set 2 w_\n"),
        ("ident_reserved_param_collides", "state hold Int\n\nf tx:Int tx_:Int\n  ok otherwise\n  ensure hold.get' 1 = tx\n  ensure hold.get' 2 = tx_\nbody\n  hold.set 1 tx\n  hold.set 2 tx_\n"),
        ("ident_def_named_like_check_fn", "state hold Int\ninvariant o:Int hold.get o >= 0\n\nput k:Int v:Int\nbody\n  hold.set (k + 1) (v - 5)\n\ninv__put k:Int\n  ok otherwise\n  ensure hold.get' 9 = 1\nbody\n  hold.set 9 1\n"),
        ("ctor_self", "state hold Int\n\nf k:Int\n  need k > 0 else Self k\n  need k < 9 else Self_\n  ok otherwise\n  emit Self k\n  emit Self_ k\nbody\n  hold.set k 1\n  audit.emit (Self k)\n  audit.emit (Self_ k)\n"),
        ("ctor_underscore", "state hold Int\n\nf k:Int\n  need k > 0 else Too_Small k\n  ok otherwise\n  emit Too_Big k\nbody\n  hold.set k 1\n  audit.emit (Too_Big k)\n"),
    ] {
        let p = dir.join(format!("{name}.k"));
        std::fs::write(&p, src).unwrap();
        let r = run_module(p.to_str().unwrap(), 200, 1, 5).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(r.eg_failures.is_empty() && r.n_divergences == 0, "{name}: {r:?}");
    }
}
