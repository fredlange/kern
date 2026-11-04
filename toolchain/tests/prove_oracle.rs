//! prove.rs against kern/prove.py (docs/research/RUST_PORT_PLAN.md, R1): every guarantee of every definition in
//! build/oracle_prove.json (written by validation/oracle_prove.py), before trust propagation. Each module runs on
//! a fresh thread, because the Z3 context and the fresh-name counter are per thread, as a fresh Python process
//! is in the oracle. Statuses, claims, reasons, relied_on and first_kill must match exactly, and so must a
//! counterexample's confirmation. Its eg line may differ (Z3's model choice depends on internal term ids, which
//! follow when Python and Rust free temporaries; see the plan), but then the Rust witness, run in the
//! interpreter with the handler draws its model made, must give exactly the outcome and state its line states.
//! KERN_ORACLE_WITNESS=exact demands identical eg lines.
use std::path::Path;

use indexmap::IndexMap;
use kern::package;
use kern::prove::Prover;
use kern::pyrepr::Py;
use kern::report::Guarantee;
use serde_json::{json, Value as J};

fn py_json(p: &Py) -> J {
    match p {
        Py::None => J::Null,
        Py::Bool(b) => J::Bool(*b),
        Py::Int(n) => i64::try_from(*n).map(J::from).unwrap_or_else(|_| J::String(n.to_string())),
        Py::Str(s) => J::String(s.clone()),
        Py::Tuple(xs) | Py::List(xs) => J::Array(xs.iter().map(py_json).collect()),
    }
}

fn guarantee_json(g: &Guarantee) -> J {
    let mut m = serde_json::Map::new();
    m.insert("kind".into(), json!(g.kind));
    m.insert("expr".into(), py_json(&g.expr.to_py()));
    m.insert("clause".into(), json!(g.clause));
    m.insert("status".into(), json!(g.status.as_str()));
    if let Some(c) = &g.counterexample {
        m.insert("counterexample".into(), json!({"eg": c.eg, "emitted": c.emitted, "confirmed_by_interpreter": c.confirmed_by_interpreter}));
    }
    if let Some(r) = &g.reason {
        m.insert("reason".into(), json!(r));
    }
    if let Some(c) = &g.channel {
        m.insert("channel".into(), json!(c));
    }
    J::Object(m)
}

/// The module's dump, plus every witness's draws and whether it replays (eg line -> replays).
type Dump = (IndexMap<String, J>, IndexMap<String, bool>);

/// Does this eg line, run in the interpreter with these handler draws (then Random(0), as render_cex), give
/// exactly the outcome and after-state it states? Lines ending in "-> error: ..." aren't replayable examples.
fn replays(defs: &kern::syntax::Module, eg: &str, draws: &[kern::Int]) -> bool {
    use kern::run::{Interp, Outcome as RO, ProvideError, Value, World};
    use kern::syntax::{parse_example, Key, Outcome};
    let Ok(ex) = parse_example(eg.strip_prefix("eg ").unwrap_or(eg), None) else { return false };
    let queue = std::cell::RefCell::new(draws.iter().copied().collect::<std::collections::VecDeque<_>>());
    let provider: kern::run::Provider = Box::new(move |name: &str, a: &[Value]| {
        if let Some(v) = queue.borrow_mut().pop_front() {
            return Ok(Value::Int(v));
        }
        let n = a.first().and_then(Value::num).filter(|n| *n > 0 && name == "rand.below")
            .ok_or_else(|| ProvideError::Other("ValueError".into(), "no value".into()))?;
        Ok(Value::Int(kern::pyrandom::Random::from_int(0).randrange(0, n as i64) as kern::Int))
    });
    let mut interp = Interp::with_provider(defs, Some(provider), 0);
    let mut w = World::from_example(&ex.world, ex.now, &ex.maps);
    let args: Vec<Value> = ex.call.1.iter().map(|k| match k { Key::Int(n) => Value::Int(*n), Key::Str(s) => Value::Str(s.clone()) }).collect();
    let Ok(out) = interp.run(&ex.call.0, &args, &mut w) else { return false };
    let same_outcome = match (&out, &ex.outcome) {
        (RO::Ok, Outcome::Ok) => true,
        (RO::Fail(c, a), Outcome::Fail(c2, a2)) => c == c2 && a.iter().map(|v| v.num()).collect::<Vec<_>>() == a2.iter().map(|n| Some(*n)).collect::<Vec<_>>(),
        _ => false,
    };
    same_outcome
        && ex.after.iter().all(|(a, b)| w.bal.get(a) == Some(b))
        && ex.after_maps.iter().all(|(x, kv)| kv.iter().all(|(k, v)| w.maps.get(x).and_then(|m| m.get(k)).copied().unwrap_or(0) == *v))
}

fn module_dump(path: String) -> Result<Dump, String> {
    let defs = package::load(&path, &IndexMap::new()).map_err(|e| e.to_string())?;
    let mut out = IndexMap::new();
    let mut replay = IndexMap::new();
    for d in defs.defs.values() {
        let mut prover = Prover::new(&defs);
        let (gs, relied) = prover.prove(d, false);
        for g in &gs {
            if let Some(c) = &g.counterexample {
                let draws = prover.witness_draws.get(&c.eg).cloned().unwrap_or_default();
                replay.insert(c.eg.clone(), replays(&defs, &c.eg, &draws));
            }
        }
        out.insert(d.name.clone(), json!({"guarantees": gs.iter().map(guarantee_json).collect::<Vec<_>>(), "relied_on": relied}));
    }
    for d in defs.defs.values() {
        let (gs, _) = Prover::new(&defs).prove(d, true);
        out[&d.name]["first_kill"] = json!(gs.iter().map(|g| g.status.as_str()).collect::<Vec<_>>());
    }
    Ok((out, replay))
}

/// Differences, as lines; witness-only differences go to `witness`, and a Rust witness that doesn't replay to
/// what its eg line states goes to `out`.
fn diff(at: &str, py: &J, rs: &J, out: &mut Vec<String>, witness: &mut Vec<String>, replay: &IndexMap<String, bool>) {
    match (py, rs) {
        (J::Object(a), J::Object(b)) => {
            if a.contains_key("eg") && a.contains_key("confirmed_by_interpreter") {
                if a["confirmed_by_interpreter"] != b.get("confirmed_by_interpreter").cloned().unwrap_or(J::Null) {
                    out.push(format!("{at}confirmed: python {} vs rust {:?}", a["confirmed_by_interpreter"], b.get("confirmed_by_interpreter")));
                }
                if a.get("eg") != b.get("eg") {
                    let eg = b.get("eg").and_then(J::as_str).unwrap_or("");
                    let ok = replay.get(eg).copied().unwrap_or(false);
                    let line = format!("{at}eg: python {} vs rust {} (rust's replays: {ok})", a["eg"], b["eg"]);
                    // an unconfirmed witness need not replay as a refutation, but its stated run must be real
                    if ok || eg.contains("-> error:") { witness.push(line) } else { out.push(line) }
                }
                return;
            }
            let ka: Vec<&String> = a.keys().collect();
            let kb: Vec<&String> = b.keys().collect();
            let (mut sa, mut sb) = (ka.clone(), kb.clone());
            sa.sort();
            sb.sort();
            if sa != sb {
                out.push(format!("{at}keys: python {sa:?} vs rust {sb:?}"));
                return;
            }
            for k in ka {
                diff(&format!("{at}{k}."), &a[k], &b[k], out, witness, replay);
            }
        }
        (J::Array(a), J::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!("{}: {} items vs {}", at.trim_end_matches('.'), a.len(), b.len()));
                return;
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff(&format!("{at}{i}."), x, y, out, witness, replay);
            }
        }
        _ => {
            if py != rs {
                out.push(format!("{}: python {} vs rust {}", at.trim_end_matches('.'), py, rs));
            }
        }
    }
}

/// Results on Z3's step-limit border, which flip with solver history (the plan's fallback): (module, field,
/// python, rust, evidence). Each was checked by hand; anything else that differs is a bug in the port.
const BORDERLINE: &[(&str, &str, &str, &str, &str)] = &[(
    "tests/probes/fz2.k",
    "f.first_kill.2",
    "\"unknown\"",
    "\"unconfirmed\"",
    "Python's first_kill check of this goal used exactly the 10,000,000-step rlimit and gave up; Python's own \
     full run of the same goal (guarantees.2) is unconfirmed, as Rust's is; neither status is a kill",
)];

fn borderline(rel: &str, line: &str) -> bool {
    BORDERLINE.iter().any(|(m, f, py, rs, _)| rel == *m && line == format!("{f}: python {py} vs rust {rs}"))
}

#[test]
fn prove_matches_python() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let file = root.join("build/oracle_prove.json");
    // build/ is generated and not committed: a clean checkout has no oracle yet. Say so loudly, and pass.
    let Ok(text) = std::fs::read_to_string(&file) else {
        eprintln!("SKIPPED: {} missing; run .venv/bin/python validation/oracle_prove.py build/oracle_prove.json", file.display());
        return;
    };
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let strict_witness = std::env::var("KERN_ORACLE_WITNESS").as_deref() == Ok("exact");
    let only = std::env::var("KERN_ORACLE_ONLY").ok();
    let (mut bad, mut witness_only, mut n_defs, mut n_goals) = (vec![], vec![], 0, 0);
    for m in &oracle {
        let rel = m["path"].as_str().unwrap().to_string();
        if only.as_ref().is_some_and(|o| !rel.contains(o.as_str())) {
            continue;
        }
        let path = root.join(&rel).to_string_lossy().to_string();
        let got = std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(move || module_dump(path))
            .unwrap()
            .join()
            .map_err(|e| format!("panic: {:?}", e.downcast_ref::<String>().cloned().or(e.downcast_ref::<&str>().map(|s| s.to_string()))));
        let (mut out, mut witness) = (vec![], vec![]);
        match (m.get("error"), got) {
            (Some(_), Ok(Err(_))) => {}
            (Some(e), Ok(Ok(_))) => out.push(format!("python failed ({e}) but rust loaded it")),
            (None, Ok(Err(e))) => out.push(format!("rust failed to load: {e}")),
            (_, Err(p)) => out.push(format!("rust {p}")),
            (None, Ok(Ok((rs, replay)))) => {
                let py = m["defs"].as_object().unwrap();
                let names: Vec<&String> = py.keys().collect();
                let rnames: Vec<&String> = rs.keys().collect();
                if names != rnames {
                    out.push(format!("definitions: python {names:?} vs rust {rnames:?}"));
                }
                for (n, pv) in py {
                    n_defs += 1;
                    n_goals += pv["guarantees"].as_array().map_or(0, |g| g.len());
                    if let Some(rv) = rs.get(n) {
                        diff(&format!("{n}."), pv, rv, &mut out, &mut witness, &replay);
                    }
                }
            }
        }
        if strict_witness {
            out.append(&mut witness);
        } else if !witness.is_empty() {
            witness_only.push(format!("{rel}\n    {}", witness.join("\n    ")));
        }
        out.retain(|l| !borderline(&rel, l));
        if !out.is_empty() {
            bad.push(format!("{rel}\n    {}", out.iter().take(12).cloned().collect::<Vec<_>>().join("\n    ")));
        }
    }
    eprintln!("prove oracle: {} modules, {n_defs} definitions, {n_goals} guarantees; {} differ", oracle.len(), bad.len());
    if !witness_only.is_empty() {
        eprintln!("witness-only differences ({}):\n{}", witness_only.len(), witness_only.join("\n"));
    }
    assert!(bad.is_empty(), "{} modules differ:\n{}", bad.len(), bad.join("\n"));
}
