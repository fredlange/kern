//! F5, evidence over assertion (ported from tests/test_fundamentals.py): when the prover gets no answer, the page
//! claims no proof. Python monkeypatched z3.Solver to answer "unknown"; here the `faults` feature's KERN_FAULT=unknown
//! does the same, in this test binary's own process (an env var would leak into other tests).
use kern::report::{analyse, Status};
use kern::syntax::parse_module;
use std::path::Path;

fn no_answer() {
    std::env::set_var("KERN_FAULT", "unknown");
    assert!(kern::prove::fault("unknown"), "kern-validation builds kern with the `faults` feature");
}

fn analysed(src: &str) -> indexmap::IndexMap<String, kern::report::Report> {
    let defs = parse_module(src, true).unwrap();
    analyse(&defs, false, None, None).unwrap()
}

#[test]
fn f5_prover_timeout_with_enough_random_successes_is_tested_only() {
    no_answer();
    let v4 = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("examples/ledger-tour/ledger_v4.k")).unwrap();
    let reps = analysed(&v4);
    let t = &reps["transfer"];
    assert!(t.guarantees.iter().all(|g| g.status == Status::Tested), "{:?}", t.guarantees.iter().map(|g| g.status).collect::<Vec<_>>());
    assert_eq!(kern::render::status(t), ("warn", "Tested only, not proven".to_string()));
    assert!(!t.clean && t.mutation.is_none());
}

#[test]
fn f5_prover_timeout_without_test_support_is_unknown() {
    no_answer();
    let reps = analysed("rare to:Account amount:Int\n  need amount > 2000 else Small\n  ensure ledger.balance' to = ledger.balance to + amount\nbody\n  ledger.credit to amount\n");
    let r = &reps["rare"];
    assert_eq!(r.guarantees.iter().map(|g| g.status).collect::<Vec<_>>(), vec![Status::Unknown]);
    assert_eq!(kern::render::status(r), ("warn", "Not proven: prover gave no answer".to_string()));
}
