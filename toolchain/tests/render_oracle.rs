//! views, render and baseline against Python, on the *same* reports: build/oracle_render.json and
//! build/oracle_render_mut.json (made by `.venv/bin/python validation/oracle_render.py OUT [--mutation]`) hold
//! Python's reports as JSON with what views.py, render.py and baseline.py made of them. Each report is decoded into
//! a Rust Report (the Def comes from loading the module in Rust) and rendered again. Skipped when missing.

mod common;
use common::{emit, expr, s};
use indexmap::IndexMap;
use kern::infer::Lifted;
use kern::pyrepr::Py;
use kern::report::{
    Counterexample, ExampleResult, GoalExpr, Guarantee, LoopAst, Mutation, Props, Report, Status, Violation,
};
use kern::why;
use kern::{baseline, render, views};
use serde_json::Value as J;

fn strs(j: &J) -> Vec<String> {
    j.as_array().map(|a| a.iter().map(s).collect()).unwrap_or_default()
}

fn status(j: &J) -> Status {
    match j.as_str().unwrap() {
        "proven" => Status::Proven,
        "assumed" => Status::Assumed,
        "conditional" => Status::Conditional,
        "refuted" => Status::Refuted,
        "unconfirmed" => Status::Unconfirmed,
        "vacuous" => Status::Vacuous,
        "unknown" => Status::Unknown,
        "tested" => Status::Tested,
        x => panic!("status {x}"),
    }
}

fn goal_expr(kind: &str, j: &J) -> GoalExpr {
    match kind {
        "ok" => GoalExpr::None,
        "keep" | "ensure" => GoalExpr::Expr(expr(j)),
        "invariant" => GoalExpr::Invariant(s(&j[0]), s(&j[1]), expr(&j[2])),
        "emit" if j.get(0).and_then(|x| x.as_str()) == Some("channel") => {
            GoalExpr::ChannelEmits(s(&j[1]), j[2].as_array().unwrap().iter().map(emit).collect())
        }
        "emit" => GoalExpr::Emits(j.as_array().unwrap().iter().map(emit).collect()),
        "loop" => GoalExpr::Loop(
            s(&j[0]),
            if j[1][0].as_str() == Some("module") { LoopAst::Module(s(&j[1][1])) } else { LoopAst::Inv(expr(&j[1])) },
        ),
        "pre" => GoalExpr::Pre(s(&j[0]), expr(&j[1])),
        k => panic!("goal kind {k}"),
    }
}

fn py(j: &J) -> Py {
    match j {
        J::Null => Py::None,
        J::Bool(b) => Py::Bool(*b),
        J::Number(n) => Py::Int(n.as_i64().unwrap() as i128),
        J::String(x) => Py::Str(x.clone()),
        J::Array(a) => Py::List(a.iter().map(py).collect()),
        J::Object(_) => panic!("object"),
    }
}

fn guarantee(g: &J) -> Guarantee {
    let kind = s(&g["kind"]);
    Guarantee {
        expr: goal_expr(&kind, &g["expr"]),
        kind,
        clause: s(&g["clause"]),
        status: status(&g["status"]),
        counterexample: g.get("counterexample").filter(|c| !c.is_null()).map(|c| Counterexample {
            eg: s(&c["eg"]),
            emitted: c["emitted"].as_str().unwrap_or("").to_string(),
            confirmed_by_interpreter: c["confirmed_by_interpreter"].as_bool().unwrap(),
        }),
        blocked_by: g.get("blocked_by").map(strs),
        assuming: g.get("assuming").map(strs),
        via_broken_spec: g.get("via_broken_spec").and_then(|x| x.as_bool()),
        reason: g.get("reason").and_then(|x| x.as_str()).map(String::from),
        vacuity: g.get("vacuity").and_then(|x| x.as_str()).map(String::from),
        tested: g.get("tested").and_then(|x| x.as_u64()),
        channel: g.get("channel").and_then(|x| x.as_str()).map(String::from),
    }
}

fn report(j: &J, def: kern::syntax::Def) -> Report {
    let u = |k: &str| j[k].as_u64().unwrap();
    let _ = u;
    Report {
        def,
        hash: s(&j["hash"]),
        uses: strs(&j["uses"]).into_iter().collect(),
        fails: j["fails"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), strs(v))).collect(),
        writes: strs(&j["writes"]).into_iter().collect(),
        lifted: j["lifted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| Lifted {
                cond: expr(&l[0]),
                ctor: s(&l[1]),
                payload: l[2].as_array().map(|p| p.iter().map(expr).collect()),
                origin: s(&l[3]),
            })
            .collect(),
        callees: strs(&j["callees"]),
        relied_on: strs(&j["relied_on"]),
        guarantees: j["guarantees"].as_array().unwrap().iter().map(guarantee).collect(),
        examples: j["examples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| ExampleResult { src: s(&e["src"]), pass: e["pass"].as_bool().unwrap(), got: s(&e["got"]) })
            .collect(),
        props: Props {
            runs: j["props"]["runs"].as_u64().unwrap(),
            succeeded: j["props"]["succeeded"].as_u64().unwrap(),
            failures: j["props"]["failures"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap())).collect(),
            violations: j["props"]["violations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| match v.get("clause") {
                    Some(c) => Violation::Clause { clause: s(c), eg: s(&v["eg"]) },
                    None => Violation::Error {
                        error: s(&v["error"]),
                        args: v["args"].as_array().unwrap().iter().map(py).collect(),
                    },
                })
                .collect(),
        },
        cached: false,
        assumptions: strs(&j["assumptions"]),
        handler_sat: j["handler_sat"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), s(v))).collect()).unwrap_or_default(),
        mutation: j.get("mutation").filter(|m| !m.is_null()).map(|m| Mutation {
            total: m["total"].as_u64().unwrap(),
            guarantee_killed: m["guarantee_killed"].as_u64().unwrap(),
            prover_killed: m["prover_killed"].as_u64().unwrap(),
            props_killed: m["props_killed"].as_u64().unwrap(),
            example_killed: m["example_killed"].as_u64().unwrap(),
            killed: m["killed"].as_u64().unwrap(),
            guarantee_missed: m["guarantee_missed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| (s(&x[0]), x[1].as_bool().unwrap()))
                .collect(),
            survived: strs(&m["survived"]),
            ill_formed: strs(&m["ill_formed"]),
            errors: strs(&m["errors"]),
        }),
        mutation_note: j.get("mutation_note").and_then(|x| x.as_str()).map(String::from),
        clean: j["clean"].as_bool().unwrap(),
        regression: strs(&j["regression"]),
    }
}

fn why_status(j: &J) -> why::Status {
    why::Status {
        count: j["count"].as_u64().unwrap() as usize,
        stale: j["stale"].as_bool().unwrap(),
        entries: strs(&j["entries"]),
        anchored_to: j.get("anchored_to").and_then(|x| x.as_str()).map(String::from),
        current: j.get("current").and_then(|x| x.as_str()).map(String::from),
        obligations: strs(&j["obligations"]),
    }
}

fn check_file(file: &str, bad: &mut Vec<String>) -> usize {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let Ok(text) = std::fs::read_to_string(format!("{root}/build/{file}")) else {
        eprintln!("SKIP: no build/{file}; run validation/oracle_render.py");
        return 0;
    };
    std::env::set_current_dir(root).unwrap(); // the terminal prints package files relative to the cwd
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let mut n = 0;
    let diff = |what: String, want: &J, got: &str, bad: &mut Vec<String>| {
        if want.as_str() != Some(got) {
            let w = want.as_str().unwrap_or("");
            let at = w.chars().zip(got.chars()).take_while(|(a, b)| a == b).count();
            let ctx: String = w.chars().skip(at.saturating_sub(60)).take(160).collect();
            let gctx: String = got.chars().skip(at.saturating_sub(60)).take(160).collect();
            bad.push(format!("{what} (first difference at char {at})\n  want …{ctx}\n  got  …{gctx}"));
        }
    };
    for m in &oracle {
        if m.get("error").is_some() {
            continue;
        }
        let path = format!("{root}/{}", s(&m["path"]));
        if !std::path::Path::new(&path).exists() {
            eprintln!("SKIP {path}: not in this checkout");
            continue;
        }
        n += 1;
        let defs = kern::package::load(&path, &IndexMap::new()).unwrap();
        let mut reports: IndexMap<String, Report> = m["reports"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, r)| (name.clone(), report(r, defs.defs[name].clone())))
            .collect();
        let whys: IndexMap<String, why::Status> =
            m["whys"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), why_status(v))).collect();
        let orphans: render::Orphans = m["orphans"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), strs(v))).collect();
        let at = s(&m["path"]);
        for (name, r) in &reports {
            let v = &m["reports"][name]["views"];
            diff(format!("{at} {name} iface"), &v["iface"], &views::iface(r, &whys[name]), bad);
            diff(format!("{at} {name} spec"), &v["spec"], &views::spec(r, &whys[name]), bad);
            diff(format!("{at} {name} editor"), &v["editor"], &views::editor(&reports, name, &whys), bad);
        }
        let title = s(&m["title"]);
        diff(format!("{at} terminal"), &m["terminal"], &render::terminal(&reports, &defs, &whys, &orphans), bad);
        diff(format!("{at} page"), &m["page"], &render::page(&reports, &defs, &whys, &title, &orphans, None), bad);
        diff(format!("{at} baseline"), &m["baseline"], &baseline::text(&reports, &defs), bad);
        let rg = &m["regressed"];
        let base = baseline::parse(rg["base"].as_str().unwrap());
        baseline::compare(&mut reports, &defs, &base);
        for (name, r) in &reports {
            if serde_json::json!(r.regression) != rg["regression"][name] {
                bad.push(format!("{at} {name} regression\n  want {}\n  got  {:?}", rg["regression"][name], r.regression));
            }
        }
        diff(format!("{at} terminal after compare"), &rg["terminal"], &render::terminal(&reports, &defs, &whys, &orphans), bad);
        diff(format!("{at} page after compare"), &rg["page"], &render::page(&reports, &defs, &whys, &title, &orphans, None), bad);
    }
    n
}

#[test]
fn views_render_and_baseline_match_python() {
    let mut bad = vec![];
    let a = check_file("oracle_render.json", &mut bad);
    let b = check_file("oracle_render_mut.json", &mut bad);
    eprintln!("{a} + {b} modules compared (without / with mutation testing), {} mismatches", bad.len());
    for x in bad.iter().take(20) {
        eprintln!("{x}");
    }
    assert!(bad.is_empty());
}
