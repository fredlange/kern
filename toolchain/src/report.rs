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
use crate::syntax::{Def, Emit, Expr};

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
    /// "not measured (--quick)" when clean and not measured
    pub mutation_note: Option<String>,
    pub clean: bool,
    /// filled by the baseline comparison (`check` against a .strength file)
    pub regression: Vec<Py>,
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
