//! The results digests (port of validation/digest.py): one hash over every clean flag, guarantee status,
//! counterexample-confirmed flag (and, for S2, mutation score) of a fixed set of modules, the floor for prover
//! changes. The text hashed is Python's `json.dumps(out, sort_keys=True)` of the same structure (pyjson.rs).
//!
//! ```text
//! digest            the S2 digest: 9 example modules + one C6 chain module, full mutation testing
//! digest --loops    loops, handlers and packages: statuses and clean flags, no mutation testing
//! digest ... --dump FILE.json   also write what was hashed
//! ```

use std::path::Path;

use indexmap::IndexMap;
use kern::report::{analyse, Report};
use serde_json::{json, Value as J};

pub const MODULES: [&str; 9] = ["escrow_s1", "escrow_s2", "escrow_s3", "ledger_v3", "ledger_v4", "ledger_v5", "ledger_v6", "ledger_v7", "ledger_large"];

pub const LOOPS: [&str; 14] = [
    "examples/batch/batch.k",
    "examples/raffle/raffle.k",
    "tests/probes/p1.k",
    "tests/probes/p2.k",
    "tests/probes/p3.k",
    "tests/probes/p4.k",
    "tests/probes/p5.k",
    "tests/probes/p6.k",
    "tests/probes/p7.k",
    "tests/probes/fz1.k",
    "tests/probes/fz2.k",
    "examples/shop",
    "examples/library",
    "examples/lottery",
];

fn project(module: &str) -> &'static str {
    if module.starts_with("escrow") {
        "escrow"
    } else if module.starts_with("ledger_v") {
        "ledger-tour"
    } else {
        "ledger-large"
    }
}

/// Python's sort_keys=True, recursively.
fn sorted(v: J) -> J {
    match v {
        J::Object(m) => {
            let mut items: Vec<(String, J)> = m.into_iter().collect();
            items.sort_by(|a, b| a.0.cmp(&b.0));
            J::Object(items.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        J::Array(xs) => J::Array(xs.into_iter().map(sorted).collect()),
        x => x,
    }
}

fn goals(r: &Report) -> J {
    J::Array(
        r.guarantees
            .iter()
            .map(|g| json!([g.kind, g.status.as_str(), g.counterexample.as_ref().map(|c| c.confirmed_by_interpreter)]))
            .collect(),
    )
}

fn hash(out: J) -> (String, J) {
    let out = sorted(out);
    let text = kern::pyjson::dumps(&out, None);
    (kern::pyrepr::sha_hex(&text, 16), out)
}

/// The loop/handler/package digest.
pub fn digest_loops(root: &Path) -> (String, J) {
    let mut out = serde_json::Map::new();
    for f in LOOPS {
        let defs = kern::package::load(&root.join(f).to_string_lossy(), &IndexMap::new()).unwrap_or_else(|e| panic!("{f}: {e}"));
        let reports = analyse(&defs, false, None, None).unwrap_or_else(|e| panic!("{f}: {e:?}"));
        let per: serde_json::Map<String, J> = reports.iter().map(|(n, r)| (n.clone(), json!([r.clean, goals(r)]))).collect();
        out.insert(f.to_string(), J::Object(per));
    }
    hash(J::Object(out))
}

/// The S2 digest (full mutation testing; minutes in Python, seconds here).
pub fn digest(root: &Path) -> (String, J) {
    let mut srcs: Vec<(String, String)> = MODULES
        .iter()
        .map(|m| (m.to_string(), std::fs::read_to_string(root.join("examples").join(project(m)).join(format!("{m}.k"))).unwrap()))
        .collect();
    srcs.push(("c6_small".into(), crate::chains::chains(4, 3, 4, 8, false)));
    let mut out = serde_json::Map::new();
    for (name, src) in srcs {
        let defs = kern::syntax::parse_module(&src, true).unwrap_or_else(|e| panic!("{name}: {e}"));
        let reports = analyse(&defs, true, None, None).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let per: serde_json::Map<String, J> = reports
            .iter()
            .map(|(n, r)| {
                let m = r.mutation.as_ref().map(|m| json!([m.guarantee_killed, m.total, m.killed]));
                (n.clone(), json!([r.clean, goals(r), m]))
            })
            .collect();
        out.insert(name, J::Object(per));
    }
    hash(J::Object(out))
}

pub fn main(argv: &[String]) -> i32 {
    let root = std::env::var("KERN_DATA").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    });
    let (h, out) = if argv.iter().any(|a| a == "--loops") { digest_loops(&root) } else { digest(&root) };
    if let Some(i) = argv.iter().position(|a| a == "--dump") {
        if let Some(p) = argv.get(i + 1) {
            let _ = std::fs::write(p, serde_json::to_string_pretty(&out).unwrap());
        }
    }
    println!("{h}");
    0
}
