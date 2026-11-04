//! The type checker against kern/wf.py: every module in build/oracle_wf.json (made by
//! `.venv/bin/python validation/oracle_wf.py build/oracle_wf.json`) must give the same problems in the same order,
//! and every mutant body the same problems(). Skipped (with a note) when the dump is missing.

mod common;
use common::{s, stmts};
use kern::syntax::parse_module;
use kern::wf::{module_problems, problems};
use serde_json::Value as J;

#[test]
fn type_checker_matches_python() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/oracle_wf.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("SKIP: no {path}; run validation/oracle_wf.py");
        return;
    };
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let (mut bad, mut n_mut, mut n_prob) = (Vec::<String>::new(), 0, 0);
    for m in &oracle {
        let defs = parse_module(m["src"].as_str().unwrap(), true).unwrap();
        let want: Vec<String> = m["problems"].as_array().unwrap().iter().map(s).collect();
        n_prob += want.len();
        let got = module_problems(&defs);
        if got != want {
            bad.push(format!("{}\n  want {:?}\n  got  {:?}", m["name"], want, got));
        }
        for mu in m.get("mutants").and_then(|x| x.as_array()).into_iter().flatten() {
            n_mut += 1;
            let mut d = defs.defs[mu["def"].as_str().unwrap()].clone();
            d.body = stmts(&mu["body"]);
            d.examples.clear();
            let mdefs = defs.with_def(d.clone());
            let want: Vec<String> = mu["problems"].as_array().unwrap().iter().map(s).collect();
            let got = problems(&mdefs, &d);
            if got != want {
                bad.push(format!("{} mutant {}\n  want {:?}\n  got  {:?}", m["name"], mu["desc"], want, got));
            }
        }
    }
    eprintln!("{} modules ({n_prob} problems), {n_mut} mutants compared, {} mismatches", oracle.len(), bad.len());
    for b in bad.iter().take(25) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
