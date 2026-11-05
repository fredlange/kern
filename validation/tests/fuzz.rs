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

/// Named constants (S5): every integer literal outside `eg` lines written as a `const` declared after the last
/// definition (no line moves). Plain char scan: a run of digits not inside a name.
fn hoist_literals(src: &str) -> (String, usize) {
    let (mut out, mut values, mut n) = (String::new(), std::collections::BTreeSet::new(), 0);
    for line in src.lines() {
        if line.trim_start().starts_with("eg ") {
            out.push_str(line);
        } else {
            let cs: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i < cs.len() {
                let in_name = i > 0 && (cs[i - 1].is_alphanumeric() || cs[i - 1] == '_');
                if cs[i].is_ascii_digit() && !in_name {
                    let j = (i..cs.len()).find(|&j| !cs[j].is_ascii_digit()).unwrap_or(cs.len());
                    let v: String = cs[i..j].iter().collect();
                    out.push_str(&format!("kc_{v}"));
                    values.insert(v);
                    n += 1;
                    i = j;
                } else {
                    out.push(cs[i]);
                    i += 1;
                }
            }
        }
        out.push('\n');
    }
    out.push('\n');
    for v in &values {
        out.push_str(&format!("const kc_{v} {v}\n"));
    }
    (out, n)
}

#[test]
fn every_generated_program_means_the_same_with_its_literals_as_constants() {
    // the full seed-1 generator set (300 loop + 200 handler programs): hoisting literals to constants must give
    // the same definitions (hash, spec, body, examples), so the fuzzer's verdicts carry over to programs with
    // constants without rerunning the oracle
    let (mut hoisted, mut compared) = (0, 0);
    for job in jobs(&root(), 1, 300, 200, 1, false) {
        let src = job.src.unwrap();
        let Ok(plain) = parse_module(&src, true) else { continue };
        if check_module(&plain).is_err() {
            continue;
        }
        let (text, n) = hoist_literals(&src);
        hoisted += n;
        let consts = parse_module(&text, true).unwrap_or_else(|e| panic!("{}: {}\n{text}", job.label, e.0));
        check_module(&consts).unwrap_or_else(|e| panic!("{}: {}\n{text}", job.label, e.message()));
        assert_eq!(plain.defs.keys().collect::<Vec<_>>(), consts.defs.keys().collect::<Vec<_>>(), "{}", job.label);
        for (name, d) in &plain.defs {
            let e = &consts.defs[name];
            assert_eq!(d.content_hash(), e.content_hash(), "{} {name}\n{text}", job.label);
            assert_eq!(format!("{:?}", (&d.needs, &d.ensures, &d.emits, &d.examples, &d.body)),
                       format!("{:?}", (&e.needs, &e.ensures, &e.emits, &e.examples, &e.body)), "{} {name}", job.label);
        }
        assert_eq!(format!("{:?}", plain.invariants), format!("{:?}", consts.invariants), "{}", job.label);
        compared += 1;
    }
    assert!(compared == 500 && hoisted > 4000, "compared {compared}, hoisted {hoisted}");
}

#[test]
fn a_small_sum_fuzz_run_is_clean_decided_and_deterministic() {
    // gen_sum (72d2446's sum and state-reading functions), and const/record variants: no contradiction, and no counterexample that fails to
    // replay. A wrong sign in the sum encoding gives contradictions; a wrong empty range gives unconfirmed `?`
    // goals instead (both measured with planted bugs, 2026-11-05: .party/notes/tester.md, #TH2UWV33V6).
    let run = || {
        // plus the const and record variants of the first 4 (S5: flattened at load, so the same verdicts)
        let mut js = kern_validation::fuzz::sum_jobs(5, 12, 60);
        js.extend(kern_validation::fuzz::variant_jobs(5, 4, 60));
        let results: Vec<_> = js.iter().map(check_program).collect();
        summarise(&results)
    };
    let s = run();
    assert_eq!(s["sum_well_formed"], "12/12");
    assert_eq!((s["const_variant_well_formed"].clone(), s["record_variant_well_formed"].clone()), (serde_json::json!("4/4"), serde_json::json!("4/4")));
    assert_eq!(s["package_variant_well_formed"], "4/4"); // agg in a used package (b71848b)
    assert!(s["variant_mismatches"].as_array().unwrap().is_empty(), "{}", s["variant_mismatches"]);
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    assert_eq!(s["unmapped"], 0);
    let e = &s["tiers"]["ensure"];
    assert!(e["proven"].as_u64().unwrap_or(0) > 5 && e["refuted"].as_u64().unwrap_or(0) > 0, "{e}");
    for kind in ["ensure", "loop"] {
        assert_eq!(s["tiers"][kind]["unconfirmed"].as_u64().unwrap_or(0), 0, "{}", s["tiers"]);
    }
    assert_eq!(run(), s);
}

#[test]
fn a_small_text_fuzz_run_is_clean_and_deterministic() {
    // gen_text (135dfca's Text): set, join, guard, append, compare, literal joins against the joined literal.
    // Teeth, planted in zz.rs (2026-11-05): joins in reverse order -> 2,400 contradictions; length off by one -> 900.
    let run = || {
        let results: Vec<_> = kern_validation::fuzz::text_jobs(5, 30, 60).iter().map(check_program).collect();
        summarise(&results)
    };
    let s = run();
    assert_eq!(s["text_well_formed"], "30/30");
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    let e = &s["tiers"]["ensure"];
    assert!(e["proven"].as_u64().unwrap_or(0) > 30 && e["refuted"].as_u64().unwrap_or(0) > 0, "{e}");
    assert_eq!(run(), s);
}

#[test]
fn every_generated_match_is_its_if_chain_and_the_small_runs_are_clean() {
    // a8cee99 desugars `match` into an if/else chain at parse time: 300 generated matches (Int, constant, negative
    // and Text patterns, with and without `_`) parse to exactly the definitions of their hand-written chains
    let mut rnd = kern::pyrandom::Random::from_int(9);
    for i in 0..300 {
        let (m, c) = kern_validation::fuzz::gen_match(&mut rnd);
        if m.contains("    -1 ->") {
            continue; // a `-1` pattern is the literal -1, which no expression spells (`0 - 1` is a subtraction)
        }
        let (a, b) = (parse_module(&m, true).unwrap_or_else(|e| panic!("{i}: {}\n{m}", e.0)), parse_module(&c, true).unwrap());
        check_module(&a).unwrap_or_else(|e| panic!("{i}: {}\n{m}", e.message()));
        assert_eq!(a.defs["f"].content_hash(), b.defs["f"].content_hash(), "{i}\n{m}\n{c}");
        assert_eq!(format!("{:?}", a.defs["f"].body), format!("{:?}", b.defs["f"].body), "{i}");
    }
    // nested loops / variable × variable (59c9e66) and match programs through the oracle
    let mut js = kern_validation::fuzz::nest_jobs(5, 20, 60);
    js.extend(kern_validation::fuzz::match_jobs(5, 20, 60));
    let results: Vec<_> = js.iter().map(check_program).collect();
    let s = summarise(&results);
    assert_eq!((s["nest_well_formed"].clone(), s["match_well_formed"].clone()), (serde_json::json!("20/20"), serde_json::json!("20/20")));
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    assert!(s["tiers"]["ensure"]["refuted"].as_u64().unwrap_or(0) > 0 && s["tiers"]["ensure"]["proven"].as_u64().unwrap_or(0) > 20);
}

#[test]
fn a_small_tick_fuzz_run_is_clean() {
    // gen_tick (S10, e1e1a68): ticks then a store and an event, a ticking callee, ticks in a loop. Teeth, planted in
    // prove.rs (2026-11-05): a tick of +2 -> 1,423 contradictions; a ticking callee taken as not ticking -> 6,000.
    let results: Vec<_> = kern_validation::fuzz::tick_jobs(5, 30, 60).iter().map(check_program).collect();
    let s = summarise(&results);
    assert_eq!(s["tick_well_formed"], "30/30");
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    assert!(s["tiers"]["ensure"]["refuted"].as_u64().unwrap_or(0) > 0 && s["tiers"]["ensure"]["proven"].as_u64().unwrap_or(0) > 5);
}

#[test]
fn a_small_frame_fuzz_run_is_clean() {
    // gen_frame (S6's stated frame, 48dd716): `keep hold except A to B`, right or one key too narrow, and `keep
    // other`. Teeth (2026-11-05): the frame's `j >= B` planted as `j > B` -> 3,103 contradictions in 24 of 100.
    let results: Vec<_> = kern_validation::fuzz::frame_jobs(5, 30, 60).iter().map(check_program).collect();
    let s = summarise(&results);
    assert_eq!(s["frame_well_formed"], "30/30");
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    let k = &s["tiers"]["keep"];
    assert!(k["proven"].as_u64().unwrap_or(0) > 5 && k["refuted"].as_u64().unwrap_or(0) > 5, "{k}");
}

#[test]
fn a_small_computed_key_fuzz_run_is_clean() {
    // gen_ckey (4b44b2f: invariants over a computed key proven at every key). Teeth (2026-11-05): the after-state
    // side of that goal evaluated on the pre-state -> 5,688 contradictions in 35 of 100.
    let results: Vec<_> = kern_validation::fuzz::ckey_jobs(5, 30, 60).iter().map(check_program).collect();
    let s = summarise(&results);
    assert_eq!(s["ckey_well_formed"], "30/30");
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    let i = &s["tiers"]["invariant"];
    assert!(i["proven"].as_u64().unwrap_or(0) > 5 && i["refuted"].as_u64().unwrap_or(0) > 0, "{i}");
}
