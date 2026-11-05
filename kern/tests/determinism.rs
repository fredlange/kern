//! Results don't depend on what ran before (Fredrik's decision of 2026-11-05, RUST_PORT_PLAN.md): every prove() call
//! gets a fresh Z3 context and fresh names. Python's toolchain lacks this: within one process its witnesses and kill
//! counts follow earlier solver calls (CLAUDE.md, known limitations).
use std::path::Path;

use indexmap::IndexMap;
use kern::check::mutation_score;
use kern::oracle::analyse_json;
use kern::package;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

#[test]
fn analyse_twice_in_one_process_gives_the_same_reports() {
    for m in ["examples/batch/batch.k", "examples/lottery", "tests/probes/fz2.k"] {
        let p = root().join(m).to_string_lossy().to_string();
        let (a, b) = (analyse_json(&p, true), analyse_json(&p, true));
        assert_eq!(a, b, "{m}: a second run in the same process differs");
    }
}

#[test]
fn a_definition_scores_the_same_alone_and_after_its_module() {
    let p = root().join("examples/batch/batch.k").to_string_lossy().to_string();
    let defs = package::load(&p, &IndexMap::new()).unwrap();
    let alone = mutation_score(&defs, &defs.defs["advance_subs"], 150);
    let full = analyse_json(&p, true);
    let in_module = &full["defs"]["advance_subs"]["mutation"];
    assert_eq!(in_module["prover_killed"].as_u64(), Some(alone.prover_killed));
    assert_eq!(in_module["guarantee_killed"].as_u64(), Some(alone.guarantee_killed));
    assert_eq!(in_module["survived"].as_array().map(|a| a.len()), Some(alone.survived.len()));
}
