//! The soundness fuzzer (validation/src/fuzz.rs; docs/research/ROADMAP_RUN.md, FZ), ported from tests/test_fuzz.py:
//! a small run is clean and deterministic, an injected prover fault is found, and the FZ findings stay fixed.
//! The full run is `cargo run -p kern-validation --release --bin fuzz` (677 programs).
use std::path::{Path, PathBuf};
use std::process::Command;

use indexmap::IndexMap;
use kern::report::{analyse, propagate_trust, Status};
use kern::run::{Interp, RunError, World};
use kern::syntax::parse_module;
use kern::wf::check_module;
use kern_validation::fuzz::{check_program, jobs, summarise, template_mutants};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn small(seed: i128) -> serde_json::Value {
    let results: Vec<_> = jobs(&root(), seed, 15, 15, 60, false).iter().map(check_program).collect();
    summarise(&results)
}

#[test]
fn a_small_fuzz_run_has_no_contradictions_and_is_deterministic() {
    let s = small(7);
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    assert!(s["well_formed"].as_u64().unwrap() * 10 >= s["programs"].as_u64().unwrap() * 8); // FZ's bar for the generator
    assert_eq!(s["unmapped"], 0);
    assert!(s["tiers"]["loop"]["proven"].as_u64().unwrap_or(0) > 0 && s["tiers"]["loop"]["refuted"].as_u64().unwrap_or(0) > 0);
    assert_eq!(small(7), s);
}

#[test]
fn every_template_mutant_is_well_formed_and_checked_in_its_own_module() {
    let ms = template_mutants(&root());
    assert!(ms.len() > 20);
    for (lbl, m) in &ms {
        let name = lbl.split(": ").next().unwrap().rsplit(':').next().unwrap();
        assert!(m.defs.contains_key(name), "{lbl}");
        assert!(m.defs.keys().all(|n| n == name || n == "transfer" || n == "sweep_expired"), "{lbl}: {:?}", m.defs.keys());
    }
    assert!(ms.iter().any(|(_, m)| {
        let mut ks: Vec<&String> = m.defs.keys().collect();
        ks.sort();
        ks == ["sweep_elsewhere", "sweep_expired", "transfer"]
    }));
}

#[test]
fn an_injected_prover_fault_is_found() {
    // fault (b): the prover treats every assumed-handler value as 0. A subprocess, so the fault can't leak into
    // the other tests of this binary.
    let out = std::env::temp_dir().join(format!("kern_fuzz_fault_b_{}.json", std::process::id()));
    let p = Command::new(env!("CARGO_BIN_EXE_fuzz"))
        .args(["--loops", "0", "--handlers", "20", "--runs", "60", "--no-mutants", "--jobs", "2", "--fault", "b", "--save"])
        .arg(&out)
        .output()
        .unwrap();
    let s: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let _ = std::fs::remove_file(&out);
    assert_eq!(s["summary"]["fault"], "b");
    assert!(s["summary"]["contradictions"].as_u64().unwrap() > 0);
    assert_eq!(p.status.code(), Some(1));
}

#[test]
fn replay_budget_stops_a_huge_loop_quickly() {
    // FZ finding 5: a counterexample model may pick a loop over ~10^9 keys; replay must give up, not run for hours
    let defs = parse_module(&std::fs::read_to_string(root().join("tests/probes/p3.k")).unwrap(), true).unwrap();
    check_module(&defs).unwrap();
    let mut it = Interp::new(&defs);
    it.budget = Some(200_000);
    it.check_loops = true;
    let t = std::time::Instant::now();
    let mut w = World::new(IndexMap::new(), IndexMap::new(), 0);
    let r = it.run("bump", &[kern::run::Value::Int(0), kern::run::Value::Int(1_000_000_000)], &mut w);
    assert!(matches!(r, Err(RunError::Kern(ref m)) if m.contains("replay too long")), "{r:?}");
    assert!(t.elapsed().as_secs() < 5);
    let mut w = World::new(IndexMap::new(), IndexMap::new(), 0);
    assert!(Interp::new(&defs).run("bump", &[kern::run::Value::Int(0), kern::run::Value::Int(3)], &mut w).is_ok());
}

#[test]
fn the_mutant_that_hung_replay_finishes() {
    let ms = template_mutants(&root());
    let found: Vec<_> = ms.iter().filter(|(l, _)| l.starts_with("examples/batch/batch.k:advance_subs: `clock.now` → `0`")).collect();
    assert_eq!(found.len(), 1);
    let t = std::time::Instant::now();
    analyse(&found[0].1, false, None, None).unwrap();
    assert!(t.elapsed().as_secs() < 60);
}

fn fz2() -> kern::syntax::Module {
    let m = parse_module(&std::fs::read_to_string(root().join("tests/probes/fz2.k")).unwrap(), true).unwrap();
    check_module(&m).unwrap();
    m
}

#[test]
fn fz2_no_kept_goal_is_proven_while_a_sibling_invariant_is_not() {
    // FZ findings 6/7: fz2's `aux` invariant isn't kept; every other "kept" goal assumed it, so none may be ✔
    let reports = analyse(&fz2(), false, None, None).unwrap();
    let kept: Vec<_> = reports["f"].guarantees.iter().filter(|g| g.kind == "loop" && g.clause.contains("(kept by every iteration)")).collect();
    assert!(kept.iter().any(|g| g.status != Status::Proven && g.clause.starts_with("aux")));
    assert!(!kept.iter().any(|g| matches!(g.status, Status::Proven | Status::Assumed)));
}

#[test]
fn fz_a_conditional_sibling_still_blocks_a_kept_goal() {
    // FZ finding 7, independent of solver luck: a sibling that is `?` (and so becomes `!`) blocks the others.
    // Python monkeypatched the prover; here the statuses are set on the reports and trust is propagated again.
    let mut reports = analyse(&fz2(), false, None, None).unwrap();
    for g in &mut reports.get_mut("f").unwrap().guarantees {
        if g.kind == "loop" && g.clause.starts_with("aux") && g.clause.contains("kept") {
            g.status = Status::Unconfirmed;
        } else {
            g.status = Status::Proven;
            g.counterexample = None;
        }
        g.blocked_by = None;
        g.via_broken_spec = None;
    }
    for r in reports.values_mut() {
        if r.def.name != "f" {
            for g in &mut r.guarantees {
                g.status = Status::Proven;
            }
        }
    }
    propagate_trust(&mut reports);
    let kept: Vec<_> = reports["f"].guarantees.iter().filter(|g| g.kind == "loop" && g.clause.contains("kept") && !g.clause.starts_with("aux")).collect();
    assert!(!kept.is_empty() && kept.iter().all(|g| g.status == Status::Conditional), "{:?}", kept.iter().map(|g| (&g.clause[..30.min(g.clause.len())], g.status)).collect::<Vec<_>>());
}
