//! `kern oracle ...`: everything `check` concludes, as JSON, for tools and debugging.
//!
//! ```text
//! kern oracle analyse PATH [--mutation]   every report: interface, guarantees, examples, property tests,
//!                                         mutation score, the three views and the terminal output
//! kern oracle prove PATH                  every definition's raw proof results (before trust propagation)
//! kern oracle mutants PATH NAME           each body mutant of NAME and its first-kill proof statuses
//! ```
//!
//! A module that doesn't load gives {"error": "..."}. Until the 2026-11-05 cutover these dumps were compared with
//! the Python toolchain's (docs/research/RUST_PORT_PLAN.md); tests/determinism.rs still uses `analyse_json`.

use std::io::Write;

use indexmap::IndexMap;
use serde_json::{json, Value as J};

use crate::package;
use crate::pyrepr::Py;
use crate::render;
use crate::report::{analyse, examples_json, guarantee_json, mutation_json, props_json, py_json, Report};
use crate::views;
use crate::why::Status as WhyStatus;

fn report_json(r: &Report, reports: &IndexMap<String, Report>, whys: &IndexMap<String, WhyStatus>) -> J {
    let no_why = WhyStatus::default();
    let lifted: Vec<J> = r
        .lifted
        .iter()
        .map(|l| {
            py_json(&Py::Tuple(vec![
                l.cond.to_py(),
                Py::s(&l.ctor),
                match &l.payload {
                    Some(p) => Py::List(p.iter().map(|e| e.to_py()).collect()),
                    None => Py::None,
                },
                Py::s(&l.origin),
            ]))
        })
        .collect();
    let fails: serde_json::Map<String, J> = r.fails.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let sat: serde_json::Map<String, J> = r.handler_sat.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({
        "hash": r.hash,
        "uses": r.uses.iter().collect::<Vec<_>>(),
        "fails": fails,
        "writes": r.writes.iter().collect::<Vec<_>>(),
        "lifted": lifted,
        "callees": r.callees,
        "relied_on": r.relied_on,
        "guarantees": r.guarantees.iter().map(|g| guarantee_json(g, false)).collect::<Vec<_>>(),
        "examples": examples_json(&r.examples),
        "props": props_json(&r.props),
        "assumptions": r.assumptions,
        "handler_sat": sat,
        "mutation": r.mutation.as_ref().map(mutation_json),
        "mutation_note": r.mutation_note,
        "clean": r.clean,
        "regression": r.regression,
        "views": {
            "iface": views::iface(r, &no_why),
            "spec": views::spec(r, &no_why),
            "editor": views::editor(reports, &r.def.name, whys),
        },
    })
}

/// validation/oracle_analyse.py's dump(), from the Rust toolchain.
pub fn analyse_json(path: &str, mutation: bool) -> J {
    let defs = match package::load(path, &IndexMap::new()) {
        Ok(d) => d,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let reports = match analyse(&defs, mutation, None, None) {
        Ok(r) => r,
        Err(e) => return json!({"error": format!("{e:?}")}),
    };
    let whys: IndexMap<String, WhyStatus> = reports.keys().map(|n| (n.clone(), WhyStatus::default())).collect();
    let out: serde_json::Map<String, J> = reports.iter().map(|(n, r)| (n.clone(), report_json(r, &reports, &whys))).collect();
    // a module with assumed handlers: each witness carries its draws (Rust-only key), so the harness can replay it
    let mut out = out;
    if !defs.handlers.is_empty() {
        for r in out.values_mut() {
            for g in r["guarantees"].as_array_mut().unwrap() {
                if let Some(c) = g.get_mut("counterexample") {
                    let eg = c["eg"].as_str().unwrap_or("").to_string();
                    c["draws"] = json!(crate::prove::witness_draws(&eg).unwrap_or_default().iter().map(|n| *n as i64).collect::<Vec<_>>());
                }
            }
        }
    }
    json!({"defs": out, "terminal": render::terminal(&reports, &defs, &whys, &render::Orphans::new())})
}

/// validation/oracle_prove.py's dump_one(), from the Rust toolchain.
pub fn prove_json(path: &str) -> J {
    let defs = match package::load(path, &IndexMap::new()) {
        Ok(d) => d,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let mut out = serde_json::Map::new();
    for d in defs.defs.values() {
        let (gs, relied) = crate::prove::Prover::new(&defs).prove(d, false);
        out.insert(d.name.clone(), json!({"guarantees": gs.iter().map(|g| guarantee_json(g, false)).collect::<Vec<_>>(), "relied_on": relied}));
    }
    for d in defs.defs.values() {
        let (gs, _) = crate::prove::Prover::new(&defs).prove(d, true);
        out[&d.name]["first_kill"] = json!(gs.iter().map(|g| g.status.as_str()).collect::<Vec<_>>());
    }
    json!({"defs": out})
}

/// Per mutant of one definition: [description, statuses of the first_kill prove run] (debugging kill counts).
pub fn mutants_json(path: &str, name: &str) -> J {
    let defs = match package::load(path, &IndexMap::new()) {
        Ok(d) => d,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let d = &defs.defs[name];
    let mut out = vec![];
    for (desc, body) in crate::check::mutants(d) {
        let mut md = d.clone();
        md.body = body;
        let mdefs = defs.with_def(md.clone());
        if !crate::wf::problems(&mdefs, &md).is_empty() {
            continue;
        }
        let (gs, _) = crate::prove::Prover::new(&mdefs).prove(&md, true);
        out.push(json!([desc, gs.iter().map(|g| g.status.as_str()).collect::<Vec<_>>()]));
    }
    J::Array(out)
}

pub fn main(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let usage = "usage: kern oracle analyse PATH [--mutation] | kern oracle prove PATH";
    let (cmd, rest) = match argv.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => {
            let _ = writeln!(err, "{usage}");
            return 2;
        }
    };
    let mutation = rest.iter().any(|a| a == "--mutation");
    let Some(path) = rest.iter().find(|a| !a.starts_with("--")) else {
        let _ = writeln!(err, "{usage}");
        return 2;
    };
    let j = match cmd {
        "analyse" => analyse_json(path, mutation),
        "prove" => prove_json(path),
        "mutants" => mutants_json(path, rest.iter().filter(|a| !a.starts_with("--")).nth(1).map(String::as_str).unwrap_or("")),
        _ => {
            let _ = writeln!(err, "{usage}");
            return 2;
        }
    };
    let _ = writeln!(out, "{j}");
    0
}
