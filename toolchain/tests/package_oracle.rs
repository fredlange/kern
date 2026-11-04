//! The package loader against kern/package.py: build/oracle_package.json and the scratch tree build/oracle_package/
//! (made by `.venv/bin/python validation/oracle_package.py build/oracle_package.json build/oracle_package`).
//! Skipped (with a note) when missing.

use indexmap::IndexMap;
use kern::package::{load, relpath};
use serde_json::{json, Value as J};

#[test]
fn package_loader_matches_python() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../build");
    let Ok(text) = std::fs::read_to_string(format!("{dir}/oracle_package.json")) else {
        eprintln!("SKIP: no build/oracle_package.json; run validation/oracle_package.py");
        return;
    };
    let root = kern::package::abspath(&format!("{dir}/oracle_package"));
    let rel = |p: &Option<String>| p.as_ref().map(|p| relpath(p, &root));
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let mut bad = vec![];
    for want in &oracle {
        let path = format!("{root}/{}", want["load"].as_str().unwrap());
        let got = match load(&path, &IndexMap::new()) {
            Err(e) => json!({ "error": e.0.replace(&format!("{root}/"), "ROOT/").replace(&root, "ROOT") }),
            Ok(defs) => {
                let mut o = json!({
                    "defs": defs.defs.iter().map(|(n, d)| json!([n, d.hash(), d.module, rel(&d.file), d.line,
                        d.ref_origin.iter().map(|((k, i), v)| json!([[k, i], v])).collect::<Vec<_>>()])).collect::<Vec<_>>(),
                    "handlers": defs.handlers.iter().map(|(n, h)| json!([n, h.module, h.line])).collect::<Vec<_>>(),
                    "states": defs.states.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
                    "channels": defs.channels,
                    "invariants": defs.invariants.iter().map(|(v, t, _)| json!([v, t])).collect::<Vec<_>>(),
                });
                if let Some(p) = &defs.package {
                    o["package"] = json!({
                        "root": rel(&p.root), "base": relpath(&p.base, &root),
                        "files": p.files.iter().map(|(m, f)| json!([m, relpath(f, &root)])).collect::<Vec<_>>(),
                        "imports": p.imports.iter().map(|(m, i)| json!([m, i])).collect::<Vec<_>>(),
                        "origin": p.origin.iter().map(|(k, v)| json!([k, v.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>()])).collect::<Vec<_>>(),
                    });
                }
                o
            }
        };
        for (k, w) in want.as_object().unwrap() {
            if k != "load" && got.get(k) != Some(w) {
                bad.push(format!("{} {k}\n  want {w}\n  got  {}", want["load"], got.get(k).unwrap_or(&J::Null)));
            }
        }
        for k in got.as_object().unwrap().keys() {
            if want.get(k).is_none() {
                bad.push(format!("{}: unexpected {k}: {}", want["load"], got[k]));
            }
        }
    }
    eprintln!("{} loads compared, {} mismatches", oracle.len(), bad.len());
    for b in bad.iter().take(20) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
