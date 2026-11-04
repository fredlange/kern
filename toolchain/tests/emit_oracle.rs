//! The Rust emitter against kern/emit_rust.py: build/oracle_emit.json (made by `.venv/bin/python
//! validation/oracle_emit.py build/oracle_emit.json`): the same crate files byte for byte, or the same Unsupported /
//! parse error text. Skipped (with a note) when missing.

use indexmap::IndexMap;
use kern::emit_rust::{crate_files, crate_name};
use kern::package::load;
use serde_json::Value as J;

#[test]
fn emitter_matches_python() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let Ok(text) = std::fs::read_to_string(format!("{root}/build/oracle_emit.json")) else {
        eprintln!("SKIP: no build/oracle_emit.json; run validation/oracle_emit.py");
        return;
    };
    let tmp = std::env::temp_dir().join(format!("kern-emit-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let mut bad = vec![];
    for r in &oracle {
        let path = match r["path"].as_str() {
            Some(p) => format!("{root}/{p}"),
            None => {
                let p = tmp.join(format!("{}.k", r["crate"].as_str().unwrap()));
                std::fs::write(&p, r["src"].as_str().unwrap()).unwrap();
                p.to_string_lossy().into_owned()
            }
        };
        let name = crate_name(&path);
        if name != r["crate"].as_str().unwrap() {
            bad.push(format!("{path}: crate {name}, want {}", r["crate"]));
        }
        let got = match load(&path, &IndexMap::new()) {
            Err(e) => serde_json::json!({ "error": e.0 }),
            Ok(defs) => match crate_files(&defs, &name, "RUNTIME") {
                Err(u) => serde_json::json!({ "unsupported": u.0 }),
                Ok(files) => serde_json::json!({ "files": files.into_iter().map(|(k, v)| (k, J::String(v))).collect::<serde_json::Map<String, J>>() }),
            },
        };
        for k in ["files", "unsupported", "error"] {
            if r.get(k) != got.get(k) {
                let (w, g) = (r.get(k).map(|x| x.to_string()).unwrap_or_default(), got.get(k).map(|x| x.to_string()).unwrap_or_default());
                let at = w.chars().zip(g.chars()).take_while(|(a, b)| a == b).count();
                bad.push(format!("{path} {k} (first difference at {at})\n  want …{}\n  got  …{}",
                    w.chars().skip(at.saturating_sub(80)).take(200).collect::<String>(),
                    g.chars().skip(at.saturating_sub(80)).take(200).collect::<String>()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("{} modules compared, {} mismatches", oracle.len(), bad.len());
    for b in bad.iter().take(10) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
