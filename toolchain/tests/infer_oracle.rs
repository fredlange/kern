//! Inference against kern/infer.py: every definition in build/oracle_infer.json (made by
//! `.venv/bin/python validation/oracle_infer.py build/oracle_infer.json`). Skipped (with a note) when missing.

use kern::infer;
use kern::syntax::{code, parse_module};
use serde_json::{json, Value as J};

#[test]
fn inference_matches_python() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/oracle_infer.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("SKIP: no {path}; run validation/oracle_infer.py");
        return;
    };
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let (mut bad, mut n) = (Vec::<String>::new(), 0);
    for m in &oracle {
        let defs = parse_module(m["src"].as_str().unwrap(), true).unwrap();
        for want in m["defs"].as_array().unwrap() {
            n += 1;
            let d = &defs.defs[want["name"].as_str().unwrap()];
            let (u, f, w) = (infer::uses(&defs, d), infer::fails(&defs, d), infer::writes(&defs, d));
            let got = json!({
                "name": d.name,
                "called_names": infer::called_names(d),
                "uses": u.iter().collect::<Vec<_>>(),
                "fails": f.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                "writes": w.iter().collect::<Vec<_>>(),
                "lifted": infer::lifted(&defs, d).iter().map(|l| json!([
                    code(&l.cond), l.ctor, l.payload.as_ref().map(|p| p.iter().map(code).collect::<Vec<_>>()), l.origin
                ])).collect::<Vec<_>>(),
                "relevant_states": infer::relevant_states(&defs, d).iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
                "spec_functions": infer::spec_functions(&defs, d),
                "callees": infer::callees(&defs, d),
                "fmt_uses": infer::fmt_uses(&u),
                "fmt_fails": infer::fmt_fails(&f),
                "fmt_writes": infer::fmt_writes(&w, d),
            });
            for (k, wv) in want.as_object().unwrap() {
                if got.get(k) != Some(wv) {
                    bad.push(format!("{} {}.{k}\n  want {wv}\n  got  {}", m["name"], d.name, got[k]));
                }
            }
        }
    }
    eprintln!("{} modules, {n} definitions compared, {} mismatches", oracle.len(), bad.len());
    for b in bad.iter().take(25) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
