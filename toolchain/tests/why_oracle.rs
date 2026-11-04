//! why against kern/why.py: build/oracle_why.json (made by `.venv/bin/python validation/oracle_why.py
//! build/oracle_why.json`). Skipped (with a note) when missing.

use kern::syntax::parse_module;
use kern::why;
use serde_json::{json, Value as J};

#[test]
fn why_matches_python() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/oracle_why.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("SKIP: no {path}; run validation/oracle_why.py");
        return;
    };
    let o: J = serde_json::from_str(&text).unwrap();
    let mut bad = vec![];
    for m in o["anchors"].as_array().unwrap() {
        let defs = parse_module(m["src"].as_str().unwrap(), true).unwrap();
        let got: Vec<String> = defs.defs.values().map(|d| why::anchor_hash(&defs, d)).collect();
        if json!(got) != m["anchors"] {
            bad.push(format!("anchors\n  want {}\n  got  {:?}", m["anchors"], got));
        }
    }
    for p in o["status"].as_array().unwrap() {
        let defs = parse_module(p["src"].as_str().unwrap(), true).unwrap();
        let store = why::parse(p["why"].as_str().unwrap(), "x.why").unwrap();
        for (n, want) in p["status"].as_object().unwrap() {
            let s = why::status(&store, &defs.defs[n], &defs);
            let mut got = json!({"count": s.count, "stale": s.stale, "entries": s.entries, "obligations": s.obligations});
            if let (Some(a), Some(c)) = (&s.anchored_to, &s.current) {
                got["anchored_to"] = json!(a);
                got["current"] = json!(c);
            }
            if &got != want {
                bad.push(format!("status {n}\n  want {want}\n  got  {got}"));
            }
        }
        let orph: serde_json::Map<String, J> = why::orphans(&store, &defs).into_iter().map(|(k, v)| (k, json!(v))).collect();
        if J::Object(orph.clone()) != p["orphans"] {
            bad.push(format!("orphans\n  want {}\n  got  {:?}", p["orphans"], orph));
        }
    }
    for t in o["texts"].as_array().unwrap() {
        let got = match why::parse(t["text"].as_str().unwrap(), "PATH") {
            Ok(st) => json!({"store": st.iter().map(|(n, b)| json!([n, b.hash, b.entries])).collect::<Vec<_>>()}),
            Err(e) => json!({"error": e.0}),
        };
        for k in ["store", "error"] {
            if t.get(k) != got.get(k) {
                bad.push(format!("text {:?}: {k}\n  want {:?}\n  got  {:?}", t["text"], t.get(k), got.get(k)));
            }
        }
    }
    eprintln!("{} anchor modules, {} status pairs, {} texts, {} mismatches", o["anchors"].as_array().unwrap().len(),
              o["status"].as_array().unwrap().len(), o["texts"].as_array().unwrap().len(), bad.len());
    for b in bad.iter().take(20) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
