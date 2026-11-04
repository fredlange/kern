//! The random differential half of tests/test_runtime.py: every example and the probes, the interpreter against
//! the compiled binary, and the packages. Skipped without cargo.

use kern_validation::difftest::{example_modules, run_module, ROOT};

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
