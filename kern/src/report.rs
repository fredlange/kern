//! What `check` concludes about each definition (port of kern/report.py). This first part fixes the types, so
//! views.rs, render.rs and baseline.rs can be written against them while prove.rs and check.rs are ported.
//!
//! Field names are the Python report dict's keys, so code reading `r["guarantees"]` ports to `r.guarantees`.
//! A key Python sets only sometimes is an `Option` here (or `Vec` that is empty when absent, where Python's
//! code never distinguishes "absent" from "empty"); the oracle dump writes the key only when Python would.
//! The key and value sets were collected from the Python dumps of the whole oracle corpus (94 modules, 242
//! definitions, 1,388 guarantees; validation/oracle_analyse.py).

use std::collections::BTreeSet;

use indexmap::IndexMap;

use crate::infer::{Fails, Lifted};
use crate::pyrepr::Py;
use crate::syntax::{Def, Emit, Expr, Module};

/// One definition's report. Python also carries `def` (the Def) and `defs` (the module); here `def` is a field
/// and the module is passed alongside, since every report shares it.
#[derive(Clone, Debug)]
pub struct Report {
    pub def: Def,
    pub hash: String,
    pub uses: BTreeSet<String>,
    pub fails: Fails,
    pub writes: BTreeSet<String>,
    pub lifted: Vec<Lifted>,
    pub callees: Vec<String>,
    /// callee names, `invariant on STATE` and `assumed HANDLER` entries the proofs rest on, in Python's order
    pub relied_on: Vec<String>,
    pub guarantees: Vec<Guarantee>,
    pub examples: Vec<ExampleResult>,
    pub props: Props,
    /// a property of the run (results reused from .kern-cache), not of the module; the oracle leaves it out
    pub cached: bool,
    /// assumed handlers the proofs rest on, transitively, sorted
    pub assumptions: Vec<String>,
    /// per handler of the module: "satisfiable", "unsatisfiable for …" or "not checked"
    pub handler_sat: IndexMap<String, String>,
    /// measured only when the definition is clean
    pub mutation: Option<Mutation>,
    /// with `mutation`, when the definition has spec needs (S8)
    pub spec_mutation: Option<SpecMutation>,
    /// "not measured (--quick)" when clean and not measured
    pub mutation_note: Option<String>,
    pub clean: bool,
    /// filled by the baseline comparison (`check` against a .strength file)
    pub regression: Vec<String>,
}

/// The tiers on the proof page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Proven,      // ✔
    Assumed,     // ⊙ proven, assuming an assumed handler's spec
    Conditional, // ! rests on something unproven
    Refuted,     // ✗ with a counterexample the interpreter confirms
    Unconfirmed, // ? a counterexample the interpreter doesn't reproduce
    Vacuous,     // ∅ never succeeds
    Unknown,     // the prover gave up (rlimit) or can't express it
    Tested,      // ◐ no proof, random tests found nothing
    Runtime,     // ○ a module invariant with no proof and no counterexample: `kern host` checks it after every call
}

impl Status {
    /// The Python string.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proven => "proven",
            Status::Assumed => "assumed",
            Status::Conditional => "conditional",
            Status::Refuted => "refuted",
            Status::Unconfirmed => "unconfirmed",
            Status::Vacuous => "vacuous",
            Status::Unknown => "unknown",
            Status::Tested => "tested",
            Status::Runtime => "runtime",
        }
    }
}

/// One claim. `kind` is one of keep, ensure, emit, ok, invariant, loop, pre (Python strings, kept as strings
/// because views and render match on them as text).
#[derive(Clone, Debug)]
pub struct Guarantee {
    pub kind: String,
    /// Python's `expr` key: always present, its shape depends on `kind`
    pub expr: GoalExpr,
    pub clause: String,
    pub status: Status,
    /// refuted / unconfirmed (and some conditional, after trust propagation)
    pub counterexample: Option<Counterexample>,
    /// conditional: what it rests on that isn't proven (Python key present only then)
    pub blocked_by: Option<Vec<String>>,
    /// assumed: the handlers whose specs it assumes
    pub assuming: Option<Vec<String>>,
    /// conditional because the counterexample came from a broken callee spec (Python: present and True)
    pub via_broken_spec: Option<bool>,
    /// vacuous: why it never succeeds
    pub reason: Option<String>,
    /// conditional after being vacuous only because of something unproven: the old reason
    pub vacuity: Option<String>,
    /// tested: how many random runs reached success
    pub tested: Option<u64>,
    /// an unknowable emit goal on a declared channel names it (Python key "channel")
    pub channel: Option<String>,
}

/// Python's `expr` value per guarantee kind (prove.py builds these exact shapes; render.py reads them).
#[derive(Clone, Debug)]
pub enum GoalExpr {
    /// `ok otherwise`: Python None
    None,
    /// keep, ensure: the clause
    Expr(Expr),
    /// a module invariant: (variable, type, condition)
    Invariant(String, String, Expr),
    /// emit on audit: the exact event sequence
    Emits(Vec<Emit>),
    /// emit on a declared channel: Python ("channel", ch, emits)
    ChannelEmits(String, Vec<Emit>),
    /// a loop goal: Python ("entry" | "kept", ast)
    Loop(String, LoopAst),
    /// an assumed handler's need at a call site: Python (handler, need)
    Pre(String, Expr),
}

/// What a loop goal is about: one of the loop's own invariants, or a module invariant (Python ("module", text)).
#[derive(Clone, Debug)]
pub enum LoopAst {
    Inv(Expr),
    Module(String),
}

impl GoalExpr {
    /// The Python value, for the oracle dump.
    pub fn to_py(&self) -> Py {
        let emits = |es: &[Emit]| Py::List(es.iter().map(Emit::to_py).collect());
        match self {
            GoalExpr::None => Py::None,
            GoalExpr::Expr(e) => e.to_py(),
            GoalExpr::Invariant(v, t, c) => Py::Tuple(vec![Py::s(v), Py::s(t), c.to_py()]),
            GoalExpr::Emits(es) => emits(es),
            GoalExpr::ChannelEmits(ch, es) => Py::Tuple(vec![Py::s("channel"), Py::s(ch), emits(es)]),
            GoalExpr::Loop(part, ast) => Py::Tuple(vec![Py::s(part), match ast {
                LoopAst::Inv(e) => e.to_py(),
                LoopAst::Module(t) => Py::Tuple(vec![Py::s("module"), Py::s(t)]),
            }]),
            GoalExpr::Pre(h, e) => Py::Tuple(vec![Py::s(h), e.to_py()]),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Counterexample {
    /// "eg WORLD | NAME ARGS -> OUTCOME | AFTER"
    pub eg: String,
    /// the audit events the replay emitted, as text
    pub emitted: String,
    pub confirmed_by_interpreter: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExampleResult {
    pub src: String,
    pub pass: bool,
    /// "ok", "Ctor arg …" or "error: …" (Outcome::text())
    pub got: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub runs: u64,
    pub succeeded: u64,
    /// refusal constructor -> count, in first-seen order
    pub failures: IndexMap<String, u64>,
    pub violations: Vec<Violation>,
}

/// A random run that broke a guarantee, or crashed the interpreter.
#[derive(Clone, Debug, PartialEq)]
pub enum Violation {
    /// Python {"clause": "KIND CLAUSE", "eg": "eg …"}
    Clause { clause: String, eg: String },
    /// Python {"error": str(e), "args": [...]}
    Error { error: String, args: Vec<Py> },
}

/// check.py's mutation_score: body mutants and which layer caught them.
#[derive(Clone, Debug, PartialEq)]
pub struct Mutation {
    pub total: u64,
    pub guarantee_killed: u64,
    pub prover_killed: u64,
    pub props_killed: u64,
    pub example_killed: u64,
    pub killed: u64,
    /// (mutant description, caught by an example) for mutants no guarantee caught
    pub guarantee_missed: Vec<(String, bool)>,
    pub survived: Vec<String>,
    pub ill_formed: Vec<String>,
    pub errors: Vec<String>,
}

/// S8: changes to the spec's own `need`s (each dropped, each comparison flipped), and what caught them, cheapest
/// first: an example, else a guarantee (prover, then random tests). Separate from `Mutation`, which measures the
/// body, so body scores and `.strength` baselines keep their meaning.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecMutation {
    pub total: u64,
    pub by_examples: u64,
    pub by_guarantees: u64,
    /// per spec need (its code) the changes nothing caught: "dropped", "`>` → `>=`"
    pub survived: Vec<(String, Vec<String>)>,
    /// changes that make the definition ill-formed: not counted
    pub ill_formed: Vec<String>,
    pub errors: Vec<String>,
}

// ================================================================ analyse (report.py)

use crate::check::{mutation_score, proptest, run_examples, spec_mutation_score};
use crate::infer::{callees, fails, lifted, uses, writes};
use crate::prove::{handler_sat, Prover};
use crate::wf::{check_module, WellFormedError};
use serde_json::{json, Value as J};

/// Random successes needed before an unproven guarantee shows as ◐ (Python TESTED_MIN).
pub const TESTED_MIN: u64 = 50;

fn ok_status(s: Status) -> bool {
    matches!(s, Status::Proven | Status::Assumed)
}

/// The quantified module-invariant assumption about `store` is justified only if every definition that writes
/// the store proves the invariant.
fn store_kept(reports: &IndexMap<String, Report>, store: &str, exclude: Option<&str>) -> bool {
    for (n, r) in reports {
        if Some(n.as_str()) == exclude {
            continue;
        }
        let split = |w: &String| crate::infer::split_write(w, &r.def).map(|(x, _)| x.to_string());
        let writes_it = r.writes.iter().any(|w| split(w).as_deref() == Some(store))
            || (store == "ledger" && r.writes.iter().any(|w| split(w).is_none()));
        if writes_it && r.guarantees.iter().any(|g| g.kind == "invariant" && g.status != Status::Proven) {
            return false;
        }
        if writes_it
            && r.guarantees.iter().any(|g| g.kind == "loop" && g.clause.starts_with("module invariant") && g.status != Status::Proven)
        {
            return false;
        }
    }
    true
}

fn clean_of(r: &Report) -> bool {
    r.guarantees.iter().all(|g| ok_status(g.status)) && r.examples.iter().all(|e| e.pass) && r.props.violations.is_empty()
}

/// Assumed handlers a definition's proofs rest on: its own, and its callees', transitively.
fn assumptions_of(reports: &IndexMap<String, Report>, r: &Report, seen: &[String]) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = r.relied_on.iter().filter_map(|c| c.strip_prefix("assumed ").map(String::from)).collect();
    for c in &r.relied_on {
        if let Some(cr) = reports.get(c) {
            if !seen.contains(c) {
                let mut s2 = seen.to_vec();
                s2.push(r.def.name.clone());
                out.extend(assumptions_of(reports, cr, &s2));
            }
        }
    }
    out
}

/// The checker's own code: a prover fix must invalidate every cached result.
pub fn tool_hash() -> &'static str {
    static H: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    H.get_or_init(|| {
        let sources = [
            include_str!("syntax.rs"), include_str!("wf.rs"), include_str!("infer.rs"), include_str!("run.rs"),
            include_str!("prove.rs"), include_str!("zz.rs"), include_str!("check.rs"), include_str!("report.rs"),
            include_str!("pyrandom.rs"), include_str!("pyrepr.rs"), include_str!("package.rs"),
        ];
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        for s in sources {
            h.update(s.as_bytes());
        }
        format!("rust:{:x}", h.finalize())
    })
}

/// Everything d's results can depend on: every definition it reaches, with its full content.
pub fn cache_key(defs: &Module, d: &Def, part: &str) -> String {
    let (mut reach, mut todo) = (BTreeSet::new(), vec![d.name.clone()]);
    while let Some(n) = todo.pop() {
        for c in callees(defs, &defs.defs[n.as_str()]) {
            if reach.insert(c.clone()) {
                todo.push(c);
            }
        }
    }
    let mut states: Vec<(&String, &String)> = defs.states.iter().collect();
    states.sort();
    let decls = crate::pyrepr::repr(&Py::Tuple(vec![
        Py::List(states.iter().map(|(a, b)| Py::Tuple(vec![Py::s(a), Py::s(b)])).collect()),
        Py::List(defs.invariants.iter().map(|(v, t, c)| Py::Tuple(vec![Py::s(v), Py::s(t), c.to_py()])).collect()),
        Py::List(defs.channels.iter().map(|c| Py::s(c)).collect()),
    ]));
    // KERN_TOOL_SALT stands in for a different checker build (tests/cache_tool_salt.rs: a tool change must
    // invalidate every cached result, as Python's test did by patching report._TOOL); unset, it adds nothing
    let tool = format!("{}{}", tool_hash(), std::env::var("KERN_TOOL_SALT").unwrap_or_default());
    let mut parts = vec![tool, part.to_string(), decls, d.name.clone(), d.content_hash()];
    parts.extend(reach.iter().map(|n| format!("{}:{}", n, defs.defs[n.as_str()].content_hash())));
    crate::pyrepr::sha_hex(&parts.join("\n"), 64)
}

fn cache_path(cache: &str, key: &str) -> std::path::PathBuf {
    std::path::Path::new(cache).join(&key[..2]).join(key)
}

fn cache_get(cache: Option<&str>, key: &str) -> Option<J> {
    let text = std::fs::read_to_string(cache_path(cache?, key)).ok()?;
    let v: J = serde_json::from_str(&text).ok()?;
    (v.get("key").and_then(J::as_str) == Some(key)).then_some(v)     // missing or corrupt: recompute, never trust
}

fn cache_put(cache: Option<&str>, key: &str, mut value: J) {
    let Some(cache) = cache else { return };
    let path = cache_path(cache, key);
    if std::fs::create_dir_all(path.parent().unwrap()).is_err() {
        return;
    }
    value["key"] = json!(key);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, value.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ---- the JSON codec (cache and oracle dump) ------------------------------------------------------------

pub fn py_json(p: &Py) -> J {
    match p {
        Py::None => J::Null,
        Py::Bool(b) => J::Bool(*b),
        Py::Int(n) => i64::try_from(*n).map(J::from).unwrap_or_else(|_| json!({"int": n.to_string()})),
        Py::Str(s) => J::String(s.clone()),
        Py::Tuple(xs) | Py::List(xs) => J::Array(xs.iter().map(py_json).collect()),
    }
}

fn json_int(j: &J) -> Option<crate::Int> {
    j.as_i64().map(|n| n as crate::Int).or_else(|| j.get("int")?.as_str()?.parse().ok())
}

fn expr_from(j: &J) -> Option<Expr> {
    let a = j.as_array()?;
    Some(match a.first()?.as_str()? {
        "int" => Expr::Int(json_int(&a[1])?),
        "name" => Expr::Name(a[1].as_str()?.to_string(), a[2].as_bool()?),
        "bin" => Expr::Bin(static_op(a[1].as_str()?)?, Box::new(expr_from(&a[2])?), Box::new(expr_from(&a[3])?)),
        "app" => {
            let h = a[1].as_array()?;
            Expr::App(h[0].as_str()?.to_string(), h[1].as_bool()?, a[2].as_array()?.iter().map(expr_from).collect::<Option<_>>()?)
        }
        "all" => Expr::All(a[1].as_str()?.to_string(), Box::new(expr_from(&a[2])?), Box::new(expr_from(&a[3])?), Box::new(expr_from(&a[4])?)),
        "sum" => Expr::Sum(
            a[1].as_str()?.to_string(),
            Box::new(expr_from(&a[2])?),
            Box::new(expr_from(&a[3])?),
            Box::new(expr_from(&a[4])?),
            if a[5].is_null() { None } else { Some(Box::new(expr_from(&a[5])?)) },
        ),
        _ => return None,
    })
}

fn static_op(op: &str) -> Option<&'static str> {
    ["+", "-", "*", "/", "=", "!=", ">=", "<=", ">", "<", "and", "or", "when"].into_iter().find(|o| *o == op)
}

fn emit_from(j: &J) -> Option<Emit> {
    let a = j.as_array()?;
    let args = a.get(1)?.as_array()?.iter().map(expr_from).collect::<Option<Vec<_>>>()?;
    let cond = match a.get(2) {
        Some(c) => Some(expr_from(c)?),
        None => None,
    };
    Some(Emit::Ev(a[0].as_str()?.to_string(), args, cond))
}

fn goal_expr_json(g: &GoalExpr) -> J {
    let t = match g {
        GoalExpr::None => "none",
        GoalExpr::Expr(_) => "expr",
        GoalExpr::Invariant(..) => "invariant",
        GoalExpr::Emits(_) => "emits",
        GoalExpr::ChannelEmits(..) => "channel",
        GoalExpr::Loop(_, LoopAst::Inv(_)) => "loop",
        GoalExpr::Loop(_, LoopAst::Module(_)) => "loop_module",
        GoalExpr::Pre(..) => "pre",
    };
    json!({"t": t, "v": py_json(&g.to_py())})
}

fn goal_expr_from(j: &J) -> Option<GoalExpr> {
    let v = &j["v"];
    Some(match j["t"].as_str()? {
        "none" => GoalExpr::None,
        "expr" => GoalExpr::Expr(expr_from(v)?),
        "invariant" => GoalExpr::Invariant(v[0].as_str()?.into(), v[1].as_str()?.into(), expr_from(&v[2])?),
        "emits" => GoalExpr::Emits(v.as_array()?.iter().map(emit_from).collect::<Option<_>>()?),
        "channel" => GoalExpr::ChannelEmits(v[1].as_str()?.into(), v[2].as_array()?.iter().map(emit_from).collect::<Option<_>>()?),
        "loop" => GoalExpr::Loop(v[0].as_str()?.into(), LoopAst::Inv(expr_from(&v[1])?)),
        "loop_module" => GoalExpr::Loop(v[0].as_str()?.into(), LoopAst::Module(v[1][1].as_str()?.into())),
        "pre" => GoalExpr::Pre(v[0].as_str()?.into(), expr_from(&v[1])?),
        _ => return None,
    })
}

fn status_from(s: &str) -> Option<Status> {
    [Status::Proven, Status::Assumed, Status::Conditional, Status::Refuted, Status::Unconfirmed, Status::Vacuous, Status::Unknown, Status::Tested,
     Status::Runtime]
        .into_iter()
        .find(|x| x.as_str() == s)
}

/// A guarantee as the Python dict (the oracle dump's shape); `cache` adds the typed `expr`.
pub fn guarantee_json(g: &Guarantee, cache: bool) -> J {
    let mut m = serde_json::Map::new();
    m.insert("kind".into(), json!(g.kind));
    m.insert("expr".into(), if cache { goal_expr_json(&g.expr) } else { py_json(&g.expr.to_py()) });
    m.insert("clause".into(), json!(g.clause));
    m.insert("status".into(), json!(g.status.as_str()));
    if let Some(c) = &g.counterexample {
        m.insert("counterexample".into(), json!({"eg": c.eg, "emitted": c.emitted, "confirmed_by_interpreter": c.confirmed_by_interpreter}));
    }
    if let Some(x) = &g.blocked_by {
        m.insert("blocked_by".into(), json!(x));
    }
    if let Some(x) = &g.assuming {
        m.insert("assuming".into(), json!(x));
    }
    if let Some(x) = g.via_broken_spec {
        m.insert("via_broken_spec".into(), json!(x));
    }
    if let Some(x) = &g.reason {
        m.insert("reason".into(), json!(x));
    }
    if let Some(x) = &g.vacuity {
        m.insert("vacuity".into(), json!(x));
    }
    if let Some(x) = g.tested {
        m.insert("tested".into(), json!(x));
    }
    if let Some(x) = &g.channel {
        m.insert("channel".into(), json!(x));
    }
    J::Object(m)
}

fn guarantee_from(j: &J) -> Option<Guarantee> {
    let strs = |k: &str| j.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
    Some(Guarantee {
        kind: j["kind"].as_str()?.into(),
        expr: goal_expr_from(&j["expr"])?,
        clause: j["clause"].as_str()?.into(),
        status: status_from(j["status"].as_str()?)?,
        counterexample: match j.get("counterexample") {
            Some(c) => Some(Counterexample {
                eg: c["eg"].as_str()?.into(),
                emitted: c["emitted"].as_str()?.into(),
                confirmed_by_interpreter: c["confirmed_by_interpreter"].as_bool()?,
            }),
            None => None,
        },
        blocked_by: strs("blocked_by"),
        assuming: strs("assuming"),
        via_broken_spec: j.get("via_broken_spec").and_then(J::as_bool),
        reason: j.get("reason").and_then(J::as_str).map(String::from),
        vacuity: j.get("vacuity").and_then(J::as_str).map(String::from),
        tested: j.get("tested").and_then(J::as_u64),
        channel: j.get("channel").and_then(J::as_str).map(String::from),
    })
}

fn py_from(j: &J) -> Py {
    match j {
        J::Null => Py::None,
        J::Bool(b) => Py::Bool(*b),
        J::Number(n) => Py::Int(n.as_i64().unwrap_or(0) as crate::Int),
        J::String(s) => Py::Str(s.clone()),
        J::Array(a) => Py::List(a.iter().map(py_from).collect()),
        J::Object(_) => json_int(j).map(Py::Int).unwrap_or(Py::None),
    }
}

pub fn examples_json(es: &[ExampleResult]) -> J {
    J::Array(es.iter().map(|e| json!({"src": e.src, "pass": e.pass, "got": e.got})).collect())
}

pub fn props_json(p: &Props) -> J {
    let violations: Vec<J> = p
        .violations
        .iter()
        .map(|v| match v {
            Violation::Clause { clause, eg } => json!({"clause": clause, "eg": eg}),
            Violation::Error { error, args } => json!({"error": error, "args": args.iter().map(py_json).collect::<Vec<_>>()}),
        })
        .collect();
    let failures: serde_json::Map<String, J> = p.failures.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({"runs": p.runs, "succeeded": p.succeeded, "failures": failures, "violations": violations})
}

fn props_from(j: &J) -> Option<Props> {
    Some(Props {
        runs: j["runs"].as_u64()?,
        succeeded: j["succeeded"].as_u64()?,
        failures: j["failures"].as_object()?.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0))).collect(),
        violations: j["violations"]
            .as_array()?
            .iter()
            .map(|v| match v.get("clause") {
                Some(c) => Violation::Clause { clause: c.as_str().unwrap_or("").into(), eg: v["eg"].as_str().unwrap_or("").into() },
                None => Violation::Error {
                    error: v["error"].as_str().unwrap_or("").into(),
                    args: v["args"].as_array().map(|a| a.iter().map(py_from).collect()).unwrap_or_default(),
                },
            })
            .collect(),
    })
}

pub fn mutation_json(m: &Mutation) -> J {
    json!({
        "total": m.total, "guarantee_killed": m.guarantee_killed, "prover_killed": m.prover_killed,
        "props_killed": m.props_killed, "example_killed": m.example_killed, "killed": m.killed,
        "guarantee_missed": m.guarantee_missed.iter().map(|(d, e)| json!([d, e])).collect::<Vec<_>>(),
        "survived": m.survived, "ill_formed": m.ill_formed, "errors": m.errors,
    })
}

pub fn spec_mutation_json(m: &SpecMutation) -> J {
    json!({
        "total": m.total, "by_examples": m.by_examples, "by_guarantees": m.by_guarantees,
        "survived": m.survived.iter().map(|(n, cs)| json!([n, cs])).collect::<Vec<_>>(), "ill_formed": m.ill_formed,
        "errors": m.errors,
    })
}

fn spec_mutation_from(j: &J) -> Option<SpecMutation> {
    let strs = |k: &str| -> Option<Vec<String>> { Some(j[k].as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect()) };
    Some(SpecMutation {
        total: j["total"].as_u64()?,
        by_examples: j["by_examples"].as_u64()?,
        by_guarantees: j["by_guarantees"].as_u64()?,
        survived: j["survived"]
            .as_array()?
            .iter()
            .map(|x| Some((x[0].as_str()?.to_string(), x[1].as_array()?.iter().filter_map(|c| c.as_str().map(String::from)).collect())))
            .collect::<Option<Vec<_>>>()?,
        ill_formed: strs("ill_formed")?,
        errors: strs("errors")?,
    })
}

fn mutation_from(j: &J) -> Option<Mutation> {
    let strs = |k: &str| -> Option<Vec<String>> { Some(j[k].as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect()) };
    Some(Mutation {
        total: j["total"].as_u64()?,
        guarantee_killed: j["guarantee_killed"].as_u64()?,
        prover_killed: j["prover_killed"].as_u64()?,
        props_killed: j["props_killed"].as_u64()?,
        example_killed: j["example_killed"].as_u64()?,
        killed: j["killed"].as_u64()?,
        guarantee_missed: j["guarantee_missed"].as_array()?.iter().filter_map(|x| Some((x[0].as_str()?.to_string(), x[1].as_bool()?))).collect(),
        survived: strs("survived")?,
        ill_formed: strs("ill_formed")?,
        errors: strs("errors")?,
    })
}

/// Trust propagates through modular proofs, to a fixpoint: a guarantee that rests on an unproven callee, store
/// invariant or sibling loop invariant becomes `!` (conditional), with what it rests on in `blocked_by`.
pub fn propagate_trust(reports: &mut IndexMap<String, Report>) {
    let mut changed = true;
    while changed {
        changed = false;
        let names: Vec<String> = reports.keys().cloned().collect();
        for name in &names {
            let n = reports[name.as_str()].guarantees.len();
            for gi in 0..n {
                let weak = {
                    let r = &reports[name.as_str()];
                    let g = &r.guarantees[gi];
                    let own_inv = g.kind == "invariant" || (g.kind == "loop" && g.clause.starts_with("module invariant"));
                    let mut out: Vec<String> = vec![];
                    for c in &r.relied_on {
                        if c.starts_with("assumed ") {
                            continue;
                        }
                        let bad = match c.strip_prefix("invariant on ") {
                            Some(store) => !store_kept(&reports, store, if own_inv { Some(r.def.name.as_str()) } else { None }),
                            None => !clean_of(&reports[c.as_str()]),
                        };
                        if bad {
                            out.push(c.clone());
                        }
                    }
                    if (g.kind != "loop" || g.clause.contains("(kept by every iteration)")) && !crate::prove::fault("a") {
                        for (xi, x) in r.guarantees.iter().enumerate() {
                            if xi != gi && x.kind == "loop" && !ok_status(x.status) {
                                out.push(format!("loop invariant {}", x.clause));
                            }
                        }
                    }
                    let mut dedup: Vec<String> = vec![];
                    for o in out {
                        if !dedup.contains(&o) {
                            dedup.push(o);
                        }
                    }
                    dedup
                };
                let g = &mut reports.get_mut(name.as_str()).unwrap().guarantees[gi];
                if g.status == Status::Proven && !weak.is_empty() {
                    g.status = Status::Conditional;
                    g.blocked_by = Some(weak);
                    changed = true;
                } else if g.status == Status::Vacuous
                    && !weak.is_empty()
                    && !g.reason.as_deref().unwrap_or("").starts_with("no input passes its own needs")
                {
                    g.status = Status::Conditional;
                    g.blocked_by = Some(weak);
                    g.vacuity = g.reason.take();
                    changed = true;
                } else if g.status == Status::Unconfirmed && !weak.is_empty() {
                    g.status = Status::Conditional;
                    g.blocked_by = Some(weak);
                    g.via_broken_spec = Some(true);
                    changed = true;
                }
            }
        }
}
}

/// Every check for every definition (report.py's analyse). `cache` = a directory: results are reused for
/// definitions whose cache_key is unchanged. A crash of the interpreter in an example or a random test is a
/// crash of the analysis, as in Python.
pub fn analyse(
    defs: &Module,
    mutation: bool,
    baseline: Option<&crate::baseline::Baseline>,
    cache: Option<&str>,
) -> Result<IndexMap<String, Report>, WellFormedError> {
    check_module(defs)?;
    let mut reports: IndexMap<String, Report> = IndexMap::new();
    for d in defs.defs.values() {
        let key = cache_key(defs, d, "base");
        let hit = cache_get(cache, &key).and_then(|v| {
            Some((
                v["guarantees"].as_array()?.iter().map(guarantee_from).collect::<Option<Vec<_>>>()?,
                v["relied_on"].as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>(),
                v["examples"]
                    .as_array()?
                    .iter()
                    .map(|e| Some(ExampleResult { src: e["src"].as_str()?.into(), pass: e["pass"].as_bool()?, got: e["got"].as_str()?.into() }))
                    .collect::<Option<Vec<_>>>()?,
                props_from(&v["props"])?,
            ))
        });
        let cached = hit.is_some();
        let (guarantees, relied_on, examples, props) = match hit {
            Some(h) => h,
            None => {
                let (g, relied) = Prover::new(defs).prove(d, false);
                let ex = run_examples(defs, d).unwrap_or_else(|c| panic!("{}: {}: {}", d.name, c.0, c.1));
                let pr = proptest(defs, d, 500, 1).unwrap_or_else(|c| panic!("{}: {}: {}", d.name, c.0, c.1));
                cache_put(
                    cache,
                    &key,
                    json!({"guarantees": g.iter().map(|x| guarantee_json(x, true)).collect::<Vec<_>>(), "relied_on": relied,
                           "examples": examples_json(&ex), "props": props_json(&pr)}),
                );
                (g, relied, ex, pr)
            }
        };
        reports.insert(
            d.name.clone(),
            Report {
                def: d.clone(),
                hash: d.hash(),
                uses: uses(defs, d),
                fails: fails(defs, d),
                writes: writes(defs, d),
                lifted: lifted(defs, d),
                callees: callees(defs, d),
                relied_on,
                guarantees,
                examples,
                props,
                cached,
                assumptions: vec![],
                handler_sat: IndexMap::new(),
                mutation: None,
                spec_mutation: None,
                mutation_note: None,
                clean: false,
                regression: vec![],
            },
        );
    }
    propagate_trust(&mut reports);
    // ⊙ proven assuming an assumed handler's spec; `!` if that spec can't be met
    let sat: IndexMap<String, String> = defs.handlers.iter().map(|(n, h)| (n.clone(), handler_sat(defs, h))).collect();
    let names: Vec<String> = reports.keys().cloned().collect();
    for name in &names {
        let assumed: Vec<String> = assumptions_of(&reports, &reports[name.as_str()], &[]).into_iter().collect();
        let r = reports.get_mut(name.as_str()).unwrap();
        r.assumptions = assumed.clone();
        r.handler_sat = sat.clone();
        let broken: Vec<&String> = assumed.iter().filter(|a| sat.get(*a).is_some_and(|s| s.starts_with("unsatisfiable"))).collect();
        for g in &mut r.guarantees {
            if g.status != Status::Proven || g.kind == "pre" || assumed.is_empty() {
                continue;
            }
            if !broken.is_empty() {
                g.status = Status::Conditional;
                g.blocked_by = Some(broken.iter().map(|a| format!("assumed {a}, whose spec can't be met")).collect());
            } else {
                g.status = Status::Assumed;
                g.assuming = Some(assumed.clone());
            }
        }
    }
    // ◐ tested-only: no proof, but random tests reached success often enough and found nothing
    for r in reports.values_mut() {
        let (succeeded, clean_props) = (r.props.succeeded, r.props.violations.is_empty());
        for g in &mut r.guarantees {
            if g.status == Status::Unknown && g.kind != "ok" && succeeded >= TESTED_MIN && clean_props {
                g.status = Status::Tested;
                g.tested = Some(succeeded);
            }
        }
    }
    // ○ runtime-checked: a module invariant the prover neither proved nor refuted is still enforced at run time,
    // since `kern host` checks the module invariants after every call (at the keys the call wrote). It keeps
    // `tested` when random tests also held it. Nothing else is checked at run time, so nothing else gets ○.
    // A random run that broke it is a counterexample the interpreter found, so the goal is ✗ with that run as its
    // `eg` line (a success, so `-> ok`), never ○ (reviewer, S11).
    for r in reports.values_mut() {
        let violations = r.props.violations.clone();
        for g in &mut r.guarantees {
            if g.kind == "invariant" && matches!(g.status, Status::Unknown | Status::Tested) {
                let broke = violations.iter().find_map(|v| match v {
                    Violation::Clause { clause, eg } if *clause == format!("invariant {}", g.clause) => Some(eg.clone()),
                    _ => None,
                });
                match broke {
                    Some(eg) => {
                        g.status = Status::Refuted;
                        g.tested = None;
                        g.counterexample =
                            Some(Counterexample { eg: format!("{eg} -> ok"), emitted: String::new(), confirmed_by_interpreter: true });
                    }
                    None => g.status = Status::Runtime,
                }
            }
        }
    }
    for name in &names {
        let is_clean = clean_of(&reports[name.as_str()]);
        let (mut m, mut note, mut sm) = (None, None, None);
        if is_clean {
            let d = &reports[name.as_str()].def;
            let key = cache_key(defs, d, "mutation");
            if let Some(hit) = cache_get(cache, &key).and_then(|v| mutation_from(&v["mutation"])) {
                m = Some(hit);
            } else if mutation {
                let got = mutation_score(defs, d, 150);
                cache_put(cache, &key, json!({"mutation": mutation_json(&got)}));
                m = Some(got);
            } else {
                note = Some("not measured (--quick)".to_string());
            }
            if m.is_some() {
                let key = cache_key(defs, d, "spec-mutation");
                // null: no spec needs; an entry that doesn't parse is measured again
                let hit = cache_get(cache, &key).and_then(|v| match &v["spec_mutation"] {
                    J::Null => Some(None),
                    j => spec_mutation_from(j).map(Some),
                });
                if let Some(hit) = hit {
                    sm = hit;
                } else if mutation {
                    let got = spec_mutation_score(defs, d, 150);
                    cache_put(cache, &key, json!({"spec_mutation": got.as_ref().map(spec_mutation_json)}));
                    sm = got;
                }
            }
        }
        let r = reports.get_mut(name.as_str()).unwrap();
        r.mutation = m;
        r.spec_mutation = sm;
        r.mutation_note = note;
        r.clean = is_clean;
    }
    // a lifted condition rewritten through a callee's spec is shown (views, page) only if that spec is clean: an
    // unmarked line must never rest on something unproven (F5). The prover relies on them through `relied_on`.
    let clean: BTreeSet<String> = reports.iter().filter(|(_, r)| r.clean).map(|(n, _)| n.clone()).collect();
    for r in reports.values_mut() {
        r.lifted.retain(|l| l.rests_on.iter().all(|x| clean.contains(x)));
    }
    for r in reports.values_mut() {
        r.regression = vec![];
    }
    if let Some(base) = baseline {
        crate::baseline::compare(&mut reports, defs, base);
    }
    Ok(reports)
}
