//! Symbolic execution to Z3 (port of kern/prove.py). Calls to other definitions are verified *modularly*: the
//! prover uses only the callee's spec (needs, ensures, keeps), never its body.
//!
//! Parity with Python (docs/research/RUST_PORT_PLAN.md, R1) needs the solver to see the same input, so this file
//! follows prove.py step for step:
//! - terms are built through zz.rs, in Python's evaluation order;
//! - fresh names come from one process-wide counter, as `itertools.count()` does;
//! - a path's condition list, refusals, opaque refusers and draws are shared lists. Python's `bind` aliases them,
//!   and a goal's premise is the path's list as it stands at solve time (a later clause's function call can
//!   still extend it), so they are `Rc<RefCell<…>>` here.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;

use indexmap::IndexMap;
use z3::ast::{Seq, Array, Bool, Dynamic, Int as ZInt, String as ZStr};
use z3::Model;

use crate::infer::{callees, fails, ledger_writes, lifted, names, relevant_states, state_writes, uses, writes, ANY_ACCOUNT};
use crate::report::{Counterexample, GoalExpr, Guarantee, LoopAst, Status};
use crate::run::{assumed_op, channel_op, emit_channels, state_op, Interp, Outcome, ProvideError, RunError, Value, World};
use crate::syntax::{call_stmt, code, emit_code, flat, Def, Emit, Expr, Key, Module, Need, Stmt};
use crate::zz;
use crate::Int;

/// Loop iterations + quantifier steps one counterexample replay may take (Python REPLAY_BUDGET).
pub const REPLAY_BUDGET: Int = 200_000;

thread_local! {
    static FRESH: Cell<u64> = const { Cell::new(0) };
    /// eg line -> handler draws, for every witness rendered on this thread (the oracle replays witnesses of
    /// modules with assumed handlers with them; Python's eg line doesn't record draws).
    static DRAWS: RefCell<IndexMap<String, Vec<Int>>> = RefCell::new(IndexMap::new());
}

/// Whether each proof gets a fresh Z3 context (KERN_Z3_FRESH). The default, `all`: a fresh context per prove call
/// and the fresh-name counter restarts, so a proof's result can't depend on anything proved before it (Fredrik,
/// 2026-11-05: "Python behaviour is not something we inherit"). `ctx`: a fresh context only. `off`: one context
/// per thread, as kern/prove.py runs (for comparing against Python's history-dependent results).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fresh {
    Off,
    Ctx,
    All,
}

pub fn fresh_mode() -> Fresh {
    match std::env::var("KERN_Z3_FRESH").as_deref() {
        Ok("ctx") => Fresh::Ctx,
        Ok("off") => Fresh::Off,
        _ => Fresh::All,
    }
}

/// Run f with a brand-new Z3 context as the thread's context, then put the old one back. The z3 crate only offers
/// this for Send + Sync closures; f runs synchronously on this thread, so asserting it is sound here.
fn in_fresh_context<R>(f: impl FnOnce() -> R) -> R {
    struct Unchecked<T>(T);
    unsafe impl<T> Send for Unchecked<T> {}
    unsafe impl<T> Sync for Unchecked<T> {}
    let ctx = unsafe {
        zz::setup();
        let cfg = z3_sys::Z3_mk_config().expect("Z3 config");
        // the step limit on the context too: a solver's own `rlimit` doesn't bound every quantified query (a
        // mutant's loop goal ran for minutes in-process while `z3 rlimit=…` gives up in about a second)
        let (k, v) = (std::ffi::CString::new("rlimit").unwrap(), std::ffi::CString::new(zz::RLIMIT.to_string()).unwrap());
        z3_sys::Z3_set_param_value(cfg, k.as_ptr(), v.as_ptr());
        let raw = z3_sys::Z3_mk_context_rc(cfg).expect("Z3 context");
        z3_sys::Z3_del_config(cfg);
        z3_sys::Z3_set_error_handler(raw, None);
        z3::Context::from_raw(raw)
    };
    let f = Unchecked(f);
    let out = z3::with_z3_context(&ctx, move || {
        let f = f;
        Unchecked((f.0)())
    });
    out.0
}

/// An injected prover fault (the fuzzer's teeth check): with the `faults` feature and KERN_FAULT=c, fault `c` is on.
///   a: trust propagation ignores a loop goal's unproven sibling invariants (report.rs)
///   b: an assumed handler's value is taken as 0 instead of any value its spec allows
///   c: the loop exits at HI instead of max(LO, HI)
///   unknown: every solver check answers "unknown" (Optimize excepted), as if each hit its step limit
pub fn fault(c: &str) -> bool {
    cfg!(feature = "faults") && std::env::var("KERN_FAULT").as_deref() == Ok(c)
}

/// The handler draws behind a witness eg line rendered on this thread, if any.
pub fn witness_draws(eg: &str) -> Option<Vec<Int>> {
    DRAWS.with(|d| d.borrow().get(eg).cloned())
}

/// `next(_fresh)`: one counter for the whole process, as in Python.
fn fresh() -> u64 {
    FRESH.with(|c| {
        let n = c.get();
        c.set(n + 1);
        n
    })
}

fn fresh_int(p: &str) -> ZInt {
    zz::int(&format!("{}{}", p, fresh()))
}

type Shared<T> = Rc<RefCell<T>>;

fn shared<T>(x: T) -> Shared<T> {
    Rc::new(RefCell::new(x))
}

/// A symbolic value: an Int or Bool term, a constructor value, or nothing (a procedure's result).
#[derive(Clone, Debug)]
enum SV {
    I(ZInt),
    B(Bool),
    /// a Text
    S(ZStr),
    /// a List (a sequence of Int)
    Q(Seq),
    Ev(String, Vec<SV>),
    Unit,
}

impl SV {
    fn i(&self) -> &ZInt {
        match self {
            SV::I(x) => x,
            other => panic!("prover: expected an Int term, got {other:?}"),
        }
    }
    fn b(&self) -> &Bool {
        match self {
            SV::B(x) => x,
            other => panic!("prover: expected a Bool term, got {other:?}"),
        }
    }
    fn s(&self) -> &ZStr {
        match self {
            SV::S(x) => x,
            other => panic!("prover: expected a Text term, got {other:?}"),
        }
    }
    fn q(&self) -> &Seq {
        match self {
            SV::Q(x) => x,
            other => panic!("prover: expected a List term, got {other:?}"),
        }
    }
    /// As a function argument, whatever its sort.
    fn dynamic(&self) -> Dynamic {
        match self {
            SV::I(x) => Dynamic::from(x.clone()),
            SV::B(x) => Dynamic::from(x.clone()),
            SV::S(x) => Dynamic::from(x.clone()),
            SV::Q(x) => Dynamic::from(x.clone()),
            other => panic!("prover: {other:?} is not a value"),
        }
    }
}

/// An appended event: ("ev", Ctor, args[, cond]) or ("opaque", who).
#[derive(Clone, Debug)]
enum Event {
    Ev(String, Vec<SV>, Option<Bool>),
    Opaque(String),
}

/// Symbolic state. `audit` holds only the events appended during the call being proved.
#[derive(Clone, Debug)]
struct Sym {
    bal: Array,
    own: Array,
    total: ZInt,
    audit: Vec<Event>,
    now: ZInt,
    maps: IndexMap<String, Array>,
    out: IndexMap<String, Vec<Event>>,
}

impl Sym {
    fn events(&self, ch: &str) -> Vec<Event> {
        if ch == "audit" {
            self.audit.clone()
        } else {
            self.out.get(ch).cloned().unwrap_or_default()
        }
    }
    fn append(&mut self, ch: &str, evs: Vec<Event>) {
        if ch == "audit" {
            self.audit.extend(evs);
        } else {
            self.out.entry(ch.to_string()).or_default().extend(evs);
        }
    }
}

fn ev_eq(x: &SV, y: &SV) -> Bool {
    match (x, y) {
        (SV::Ev(cx, ax), SV::Ev(cy, ay)) => {
            if cx != cy || ax.len() != ay.len() {
                return zz::bval(false);
            }
            let mut parts = vec![zz::bval(true)];
            parts.extend(ax.iter().zip(ay).map(|(a, b)| ev_eq(a, b)));
            zz::and(&parts)
        }
        (SV::Ev(..), _) | (_, SV::Ev(..)) => zz::bval(false),
        (SV::I(a), SV::I(b)) => zz::eq_int(a, b),
        (SV::B(a), SV::B(b)) => zz::eq_bool(a, b),
        (SV::S(a), SV::S(b)) => zz::eq_text(a, b),
        (SV::Q(a), SV::Q(b)) => zz::eq_list(a, b),
        _ => panic!("prover: can't compare {x:?} and {y:?}"),
    }
}

/// One way through a body: path condition, state, bindings, refusal points, own conditions, draws.
#[derive(Clone)]
struct Path {
    pc: Shared<Vec<Bool>>,
    s: Sym,
    env: IndexMap<String, SV>,
    refusals: Shared<Vec<(Vec<Bool>, Bool)>>,
    opaque: Shared<Vec<String>>,
    own: Vec<Bool>,
    draws: Shared<Vec<(String, ZInt)>>,
}

impl Path {
    fn fork(&self, cond: Bool) -> Path {
        let mut pc = self.pc.borrow().clone();
        pc.push(cond.clone());
        let mut own = self.own.clone();
        own.push(cond);
        Path {
            pc: shared(pc),
            s: self.s.clone(),
            env: self.env.clone(),
            refusals: shared(self.refusals.borrow().clone()),
            opaque: shared(self.opaque.borrow().clone()),
            own,
            draws: shared(self.draws.borrow().clone()),
        }
    }
}

/// One (goal, premise[, draws]) per path: the goal holds iff it holds on every path.
#[derive(Clone)]
struct Case {
    goal: Bool,
    pc: Shared<Vec<Bool>>,
    draws: Option<Shared<Vec<(String, ZInt)>>>,
}

fn snapshot(pc: &Shared<Vec<Bool>>) -> Shared<Vec<Bool>> {
    shared(pc.borrow().clone())
}

struct Goal {
    kind: String,
    expr: GoalExpr,
    text: String,
    cases: Vec<Case>,
}

fn wild(e: &Expr) -> bool {
    matches!(e, Expr::Name(n, false) if n == "_")
}

fn need_cond(n: &Need) -> &Expr {
    match n {
        Need::Need(c, _, _) => c,
        Need::As(..) => panic!("prover: `need as` should have been expanded"),
    }
}

fn emit_parts(ev: &Emit) -> (&String, &Vec<Expr>, Option<&Expr>) {
    match ev {
        Emit::Ev(c, a, cond) => (c, a, cond.as_ref()),
        Emit::As(..) => panic!("prover: `emit as` should have been expanded"),
    }
}

/// c if e is the loop variable plus a constant (k, k + c, k - c), else None.
fn loop_offset(e: &Expr, var: &str) -> Option<Int> {
    match e {
        Expr::Name(n, false) if n == var => Some(0),
        Expr::Bin(op, l, r) if (*op == "+" || *op == "-") && matches!(&**l, Expr::Name(n, false) if n == var) => {
            match &**r {
                Expr::Int(c) => Some(if *op == "+" { *c } else { -*c }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// `_loop_effects`: (ledger accounts written, maps written only at the loop key -> offset, maps fully
/// havocked, channels emitted on).
type Effects = (Vec<Option<Expr>>, IndexMap<String, Int>, BTreeSet<String>, BTreeSet<String>);

pub struct Prover<'m> {
    defs: &'m Module,
    relied_on: BTreeSet<String>,
    pc: Shared<Vec<Bool>>,
    refusals: Shared<Vec<(Vec<Bool>, Bool)>>,
    opaque_refusers: Shared<Vec<String>>,
    draws: Shared<Vec<(String, ZInt)>>,
    loop_goals: Vec<Goal>,
    call_pre: Option<Sym>,
    loop_var: String,
    replay_draws: Vec<(String, ZInt)>,
    /// eg line -> the assumed-handler values the model drew for it, in call order. Python's eg line doesn't
    /// record them; the oracle needs them to replay a witness in a module with handlers (not in the report).
    pub witness_draws: IndexMap<String, Vec<Int>>,
    /// one recursive function per distinct sum (its text, the names it reads, whether it sees an after-state)
    sums: IndexMap<String, (z3::RecFuncDecl, Option<Int>, Option<Int>)>,
    /// the definition's Text parameters, as String constants (counterexamples read them from the model)
    text_params: IndexMap<String, ZStr>,
    /// the Account parameters of the definition being proved (never `none`)
    acct_params: BTreeSet<String>,
    /// List parameters (a model gives them as sequences)
    list_params: IndexMap<String, Seq>,
    /// the `for j …` / `sum j …` ranges being evaluated, innermost last: a goal raised inside holds for each j
    ranges: Vec<(ZInt, ZInt, ZInt)>,
    /// > 0 while a sum's recursive function is built or a callee's spec assumed: no goals then
    quiet: u32,
    /// evaluating the definition's own spec after its body: goals are stated, not assumed
    in_spec: bool,
    /// while its body runs: the definition being proved and its measure at its own parameters (`decreases`)
    measuring: Option<(String, Option<ZInt>)>,
    /// scoring a mutant: how many more solver calls may come back unknown before the proof stops (each one spends
    /// the whole step limit and can't kill the mutant); None outside mutation testing
    unknowns_left: Option<u32>,
    /// the body called itself (directly): its proofs used its own spec, licensed only by its measure
    self_called: bool,
}

/// Whether evaluating `e` can raise a call-site goal: an `at`, or (in a module with Account-valued states) any call.
/// Coarse, and false for every module without lists or such states, so their proofs are as before.
fn raises_goals(defs: &Module, e: &Expr) -> bool {
    crate::infer::walk(e)
        .iter()
        .any(|n| matches!(n, Expr::App(h, _, _) if h == "at" || !defs.account_states.is_empty()))
}

/// Marks a call-site goal raised by a definition's own spec (an `at` in an ensure): the call doesn't evaluate its
/// spec, so no other guarantee rests on it.
pub const IN_SPEC: &str = "(in the spec)";

/// `none` (an unset entry of an Account-valued state) in the prover, where accounts are Ints; every Account
/// parameter is assumed to be another one.
const NONE_ID: i128 = -1;

/// A `for j from A to B` whose bounds are literals at most this far apart is expanded into a conjunction.
const EXPAND_UP_TO: i128 = 16;

/// Unknown solver answers a mutant's proof may spend before it stops (a deterministic bound: steps, not time).
pub const MUTANT_UNKNOWNS: u32 = 4;

/// Bounds [lo, hi] (None = unbounded) of an Int term over every environment and state: interval arithmetic on its
/// syntax, so a sum's lemma holds wherever it is applied. Reads, names and calls are unbounded.
fn term_bounds(e: &Expr) -> (Option<Int>, Option<Int>) {
    let both = |a: Option<Int>, b: Option<Int>, f: fn(Int, Int) -> Option<Int>| match (a, b) {
        (Some(x), Some(y)) => f(x, y),
        _ => None,
    };
    match e {
        Expr::Int(n) => (Some(*n), Some(*n)),
        Expr::Bin("+", l, r) => {
            let ((a, b), (c, d)) = (term_bounds(l), term_bounds(r));
            (both(a, c, |x, y| x.checked_add(y)), both(b, d, |x, y| x.checked_add(y)))
        }
        Expr::Bin("-", l, r) => {
            let ((a, b), (c, d)) = (term_bounds(l), term_bounds(r));
            (both(a, d, |x, y| x.checked_sub(y)), both(b, c, |x, y| x.checked_sub(y)))
        }
        Expr::Bin("*", l, r) => {
            let (k, x) = match (&**l, &**r) {
                (Expr::Int(k), x) | (x, Expr::Int(k)) => (*k, x),
                _ => return (None, None),
            };
            let (a, b) = term_bounds(x);
            let (a, b) = (a.and_then(|v| v.checked_mul(k)), b.and_then(|v| v.checked_mul(k)));
            if k >= 0 { (a, b) } else { (b, a) }
        }
        Expr::Bin("/", l, r) => match &**r {
            // floor division by a positive literal is monotone
            Expr::Int(k) if *k > 0 => {
                let (a, b) = term_bounds(l);
                (a.map(|v| v.div_euclid(*k)), b.map(|v| v.div_euclid(*k)))
            }
            _ => (None, None),
        },
        _ => (None, None),
    }
}

/// The integer literals in a definition's spec and body (keys a counterexample may need, `hold.0`).
fn literals(d: &Def) -> Vec<i128> {
    let mut exprs: Vec<&Expr> = d.ensures.iter().chain(d.keeps.iter()).collect();
    for n in &d.needs {
        exprs.push(need_cond(n));
    }
    for s in flat(&d.body) {
        match s {
            Stmt::Let(_, e) | Stmt::Do(e) | Stmt::If(e, _, _) | Stmt::Inv(e) => exprs.push(e),
            Stmt::Need(c, _, a) => {
                exprs.push(c);
                exprs.extend(a.iter());
            }
            Stmt::For(_, lo, hi, invs, _) => {
                exprs.extend([lo, hi]);
                exprs.extend(invs.iter());
            }
        }
    }
    let mut out: Vec<i128> =
        exprs.into_iter().flat_map(crate::infer::walk).filter_map(|e| if let Expr::Int(n) = e { Some(*n) } else { None }).collect();
    out.sort();
    out.dedup();
    out.truncate(20);
    out
}

/// Whether `from` can call `to` (itself included): a call from `to`'s body to `from` is then recursive.
fn reaches(defs: &Module, from: &str, to: &str) -> bool {
    let (mut seen, mut todo) = (BTreeSet::new(), vec![from.to_string()]);
    while let Some(n) = todo.pop() {
        let Some(d) = defs.defs.get(&n) else { continue };
        let mut next: Vec<String> = callees(defs, d);
        if crate::infer::called_names(d).iter().any(|x| *x == n) {
            next.push(n.clone());
        }
        for c in next {
            if c == to {
                return true;
            }
            if seen.insert(c.clone()) {
                todo.push(c);
            }
        }
    }
    false
}

/// A state's parts as function arguments: balances, owners, total, now, then each state map in declaration order.
fn state_args(s: &Sym) -> Vec<Dynamic> {
    let mut out: Vec<Dynamic> =
        vec![Dynamic::from(s.bal.clone()), Dynamic::from(s.own.clone()), Dynamic::from(s.total.clone()), Dynamic::from(s.now.clone())];
    out.extend(s.maps.values().map(|m| Dynamic::from(m.clone())));
    out
}

/// Whether a state map's array holds Text.
fn is_text_array(a: &Array) -> bool {
    zz::holds_text(a)
}

/// A state of fresh constants named after `p`, with `s`'s maps.
fn formal_state(s: &Sym, p: &str) -> Sym {
    Sym {
        bal: zz::array(&format!("{p}_bal")),
        own: zz::array(&format!("{p}_own")),
        total: zz::int(&format!("{p}_total")),
        audit: vec![],
        now: zz::int(&format!("{p}_now")),
        maps: s.maps.iter().map(|(x, a)| (x.clone(), zz::array_like(&format!("{p}_map_{x}"), a))).collect(),
        out: IndexMap::new(),
    }
}

type Env = IndexMap<String, SV>;

fn env_with(env: &Env, k: &str, v: SV) -> Env {
    let mut e = env.clone();
    e.insert(k.to_string(), v);
    e
}

impl<'m> Prover<'m> {
    pub fn new(defs: &'m Module) -> Self {
        Prover {
            defs,
            relied_on: BTreeSet::new(),
            pc: shared(vec![]),
            refusals: shared(vec![]),
            opaque_refusers: shared(vec![]),
            draws: shared(vec![]),
            loop_goals: vec![],
            call_pre: None,
            loop_var: String::new(),
            replay_draws: vec![],
            witness_draws: IndexMap::new(),
            sums: IndexMap::new(),
            text_params: IndexMap::new(),
            acct_params: BTreeSet::new(),
            list_params: IndexMap::new(),
            ranges: vec![],
            quiet: 0,
            in_spec: false,
            measuring: None,
            unknowns_left: None,
            self_called: false,
        }
    }

    fn invariants(&self) -> &'m [(String, String, Expr)] {
        &self.defs.invariants
    }

    // ---- expressions --------------------------------------------------------------------------------------

    /// Python `sev(e, env, cur, post)` for reads: nothing here changes a state.
    fn sev(&mut self, e: &Expr, env: &Env, cur: &Sym, post: Option<&Sym>) -> SV {
        match e {
            Expr::Int(n) => SV::I(zz::val(*n)),
            Expr::Text(t) => SV::S(zz::text(t)),
            Expr::All(v, a, b, body) => {
                let lo = self.sev(a, env, cur, post);
                let hi = self.sev(b, env, cur, post);
                // bounds that are small literals (a loop's exit, `for e from 0 to 7`): the conjunction itself, so a
                // key computed from j (`r * 7 + e`) is a ground term the solver can match, not a quantifier
                if let (Some(l), Some(h)) = (zz::numeral(&z3::ast::Ast::simplify(lo.i())), zz::numeral(&z3::ast::Ast::simplify(hi.i()))) {
                    if (1..=EXPAND_UP_TO).contains(&(h - l)) {
                        let parts: Vec<Bool> = (l..h)
                            .map(|i| self.sev(body, &env_with(env, v, SV::I(zz::val(i))), cur, post).b().clone())
                            .collect();
                        return SV::B(zz::and(&parts));
                    }
                }
                let j = zz::int(&format!("{}_{}", v, fresh()));
                // a goal raised inside (an `at`) holds for every j in the range
                self.ranges.push((j.clone(), lo.i().clone(), hi.i().clone()));
                let inner = self.sev(body, &env_with(env, v, SV::I(j.clone())), cur, post);
                self.ranges.pop();
                let range = zz::and(&[zz::le(lo.i(), &j), zz::lt(&j, hi.i())]);
                SV::B(zz::forall(&[&j], &zz::implies(&range, inner.b())))
            }
            Expr::Frame(x, range) => {
                let post = post.expect("a frame needs the after-state");
                let (before, after) = (&cur.maps[x.as_str()], &post.maps[x.as_str()]);
                match range {
                    None => SV::B(zz::eq_array(after, before)),
                    Some((a, b)) => {
                        let lo = self.sev(a, env, cur, None).i().clone();
                        let hi = self.sev(b, env, cur, None).i().clone();
                        let j = zz::int(&format!("j_{}", fresh()));
                        let outside = zz::or(&[zz::lt(&j, &lo), zz::ge(&j, &hi)]);
                        SV::B(zz::forall(&[&j], &zz::implies(&outside, &zz::same_at(after, before, &j))))
                    }
                }
            }
            Expr::Sum(j, a, b, body, cond) => {
                let lo = self.sev(a, env, cur, post).i().clone();
                let hi = self.sev(b, env, cur, post).i().clone();
                // the recursive function is built over formal constants: no goal can be stated there ...
                self.quiet += 1;
                let t = self.sum_term(j, body, cond.as_deref(), env, cur, post, &lo, &hi);
                self.quiet -= 1;
                // ... so a body that raises one (an `at`) is gone over again for every j in the range
                if [&**body].into_iter().chain(cond.as_deref()).any(|e| raises_goals(self.defs, e)) {
                    let jv = zz::int(&format!("{}_{}", j, fresh()));
                    self.ranges.push((jv.clone(), lo.clone(), hi.clone()));
                    let e2 = env_with(env, j, SV::I(jv));
                    if let Some(c) = cond {
                        self.sev(c, &e2, cur, post);
                    }
                    self.sev(body, &e2, cur, post);
                    self.ranges.pop();
                }
                SV::I(t)
            }
            Expr::Bin(op, l, r) => {
                let l = self.sev(l, env, cur, post);
                let r = self.sev(r, env, cur, post);
                if let (SV::Q(a), SV::Q(b)) = (&l, &r) {
                    match *op {
                        "=" => return SV::B(zz::eq_list(a, b)),
                        "!=" => return SV::B(zz::not(&zz::eq_list(a, b))),
                        other => panic!("prover: {other} on List"),
                    }
                }
                if let (SV::S(a), SV::S(b)) = (&l, &r) {
                    match *op {
                        "+" => return SV::S(zz::text_join(a, b)),
                        "=" => return SV::B(zz::eq_text(a, b)),
                        "!=" => return SV::B(zz::ne_text(a, b)),
                        other => panic!("prover: {other} on Text"),
                    }
                }
                SV::B(match *op {
                    "+" => return SV::I(zz::add(l.i(), r.i())),
                    "-" => return SV::I(zz::sub(l.i(), r.i())),
                    "*" => return SV::I(zz::mul(l.i(), r.i())),
                    "/" => return SV::I(zz::div(l.i(), r.i())),
                    "=" => match (&l, &r) {
                        (SV::B(a), SV::B(b)) => zz::eq_bool(a, b),
                        _ => zz::eq_int(l.i(), r.i()),
                    },
                    "!=" => match (&l, &r) {
                        (SV::B(a), SV::B(b)) => zz::ne_bool(a, b),
                        _ => zz::ne_int(l.i(), r.i()),
                    },
                    ">=" => zz::ge(l.i(), r.i()),
                    "<=" => zz::le(l.i(), r.i()),
                    ">" => zz::gt(l.i(), r.i()),
                    "<" => zz::lt(l.i(), r.i()),
                    "and" => zz::and(&[l.b().clone(), r.b().clone()]),
                    "or" => zz::or(&[l.b().clone(), r.b().clone()]),
                    "when" => zz::implies(r.b(), l.b()),
                    other => panic!("prover: unknown operator {other}"),
                })
            }
            Expr::Name(n, primed) => {
                if let Some(v) = env.get(n) {
                    return v.clone();
                }
                if n == crate::run::NONE {
                    return SV::I(zz::val(NONE_ID));
                }
                if n == "[]" {
                    return SV::Q(zz::list_empty());
                }
                if let Some(v) = self.field_of(env, n) {
                    return v;
                }
                let s = if *primed { post.expect("a primed read needs a post-state") } else { cur };
                self.scall_read(n, vec![], s)
            }
            Expr::App(name, primed, args) => {
                let vals: Vec<SV> = args.iter().map(|a| self.sev(a, env, cur, post)).collect();
                self.accounts_used(name, args, &vals);
                if name == "at" {
                    // outside the list the interpreter stops: a call-site goal, the index in [0, length)
                    let (xs, i) = (vals[0].q(), vals[1].i());
                    let inside = zz::and(&[zz::ge(i, &zz::val(0)), zz::lt(i, &zz::list_len(xs))]);
                    let text = format!("{} is inside the list (0 <= index < length)", code(e));
                    self.call_site_goal("at", inside, &text);
                }
                let s = if *primed { post.expect("a primed read needs a post-state") } else { cur };
                self.scall_read(name, vals, s)
            }
        }
    }

    /// `sum j from lo to hi of body [where cond]`: a recursive function of the environment values and the state(s)
    /// the body may read, applied to the actual ones. The same sum (text, names read, after-state or not) is the
    /// same function within a proof, so a loop invariant at entry and after a step talk about one function.
    #[allow(clippy::too_many_arguments)]
    fn sum_term(&mut self, j: &str, body: &Expr, cond: Option<&Expr>, env: &Env, cur: &Sym, post: Option<&Sym>,
                lo: &ZInt, hi: &ZInt) -> ZInt {
        let mut free: Vec<String> = vec![];
        for x in std::iter::once(body).chain(cond) {
            for n in crate::infer::walk(x) {
                if let Expr::Name(v, false) = n {
                    if v != j && env.contains_key(v) && !free.contains(v) {
                        free.push(v.clone());
                    }
                }
            }
        }
        // the after-state is an argument only if the sum reads it, so the same sum in a body, a loop invariant and an
        // ensure is one function
        let primed = std::iter::once(body).chain(cond).any(|x| {
            crate::infer::walk(x).iter().any(|n| matches!(n, Expr::Name(_, true) | Expr::App(_, true, _)))
        });
        let post = if primed { post } else { None };
        // the bounds are arguments, not part of the function: `sum j from lo to k` and `… to hi` must be one function
        let key = format!("{}|{}|{}|{}|{}", j, code(body), cond.map(code).unwrap_or_default(), free.join(","), post.is_some());
        let value = |v: &SV| -> Dynamic { v.dynamic() };
        if !self.sums.contains_key(&key) {
            let n = fresh();
            let p = format!("sum{n}");
            let fenv: Env = free
                .iter()
                .map(|v| {
                    let name = format!("{p}_v_{v}"); // apart from the bounds' formals below
                    let f = match &env[v] {
                        SV::B(_) => SV::B(Bool::new_const(name)),
                        SV::S(_) => SV::S(zz::text_const(&name)),
                        SV::Q(_) => SV::Q(zz::list_const(&name)),
                        _ => SV::I(zz::int(&name)),
                    };
                    (v.clone(), f)
                })
                .collect();
            let fcur = formal_state(cur, &format!("{p}_pre"));
            let fpost = post.map(|s| formal_state(s, &format!("{p}_post")));
            let (flo, fhi) = (zz::int(&format!("{p}_from")), zz::int(&format!("{p}_to")));
            let env2 = env_with(&fenv, j, SV::I(zz::sub(&fhi, &zz::val(1))));
            let x = self.sev(body, &env2, &fcur, fpost.as_ref()).i().clone();
            let step = match cond {
                Some(c) => {
                    let keep = self.sev(c, &env2, &fcur, fpost.as_ref()).b().clone();
                    zz::ite(&keep, &x, &zz::val(0))
                }
                None => x,
            };
            let mut formals: Vec<Dynamic> = free.iter().map(|v| value(&fenv[v])).collect();
            formals.extend(state_args(&fcur));
            if let Some(fp) = &fpost {
                formals.extend(state_args(fp));
            }
            let f = zz::rec_sum(&p, &formals, &flo, &fhi, &step);
            // each term's bounds over every environment and state: interval arithmetic on the syntax (a filtered
            // term can also be 0), then a validity check of `step >= 0` / `step <= 0` where that is stronger
            let (mut m, mut big_m) = term_bounds(body);
            if cond.is_some() {
                (m, big_m) = (m.map(|x| x.min(0)), big_m.map(|x| x.max(0)));
            }
            let valid = |claim: Bool| {
                let solver = zz::solver();
                solver.assert(&zz::not(&claim));
                zz::check(&solver) == zz::Check::Unsat
            };
            if m.map_or(true, |x| x < 0) && valid(zz::ge(&step, &zz::val(0))) {
                m = Some(0);
            }
            if big_m.map_or(true, |x| x > 0) && valid(zz::le(&step, &zz::val(0))) {
                big_m = Some(0);
            }
            self.sums.insert(key.clone(), (f, m, big_m));
        }
        let mut args: Vec<Dynamic> = free.iter().map(|v| value(&env[v])).collect();
        args.extend(state_args(cur));
        if let Some(p) = post {
            args.extend(state_args(p));
        }
        args.push(Dynamic::from(lo.clone()));
        args.push(Dynamic::from(hi.clone()));
        let (f, m, big_m) = &self.sums[&key];
        let (m, big_m) = (*m, *big_m);
        let total = zz::apply_rec(f, &args);
        // a sum lemma, as ground facts at this application (no induction needed): each term lies in [m, M] for
        // every environment and state, so the n = max(0, hi - lo) terms sum to [m*n, M*n]
        let n = zz::ite(&zz::gt(hi, lo), &zz::sub(hi, lo), &zz::val(0));
        if let Some(m) = m {
            self.pc.borrow_mut().push(zz::ge(&total, &zz::mul(&zz::val(m), &n)));
        }
        if let Some(big_m) = big_m {
            self.pc.borrow_mut().push(zz::le(&total, &zz::mul(&zz::val(big_m), &n)));
        }
        total
    }

    /// Python `sev` at statement level, where the call may change the state (a write, an emit, a callee).
    fn sev_mut(&mut self, e: &Expr, env: &Env, s: &mut Sym) -> SV {
        let (name, args): (&str, &[Expr]) = match e {
            Expr::App(n, false, a) => (n, a),
            Expr::Name(n, false) if !env.contains_key(n) => (n, &[]),
            _ => return self.sev(e, env, s, None),
        };
        if !self.mutates(name) {
            return self.sev(e, env, s, None);
        }
        let vals: Vec<SV> = args.iter().map(|a| self.sev(a, env, s, None)).collect();
        self.accounts_used(name, args, &vals);
        self.scall_write(name, vals, s)
    }

    /// A goal at this point of the path (its premise: the path so far), assumed afterwards: one per text, a case per
    /// place it is reached.
    fn call_site_goal(&mut self, site: &str, c: Bool, text: &str) {
        if self.quiet > 0 {
            return;
        }
        let mut c = c;
        for (j, lo, hi) in self.ranges.iter().rev() {
            let inside = zz::and(&[zz::le(lo, j), zz::lt(j, hi)]);
            c = zz::forall(&[j], &zz::implies(&inside, &c));
        }
        // raised by the definition's own spec: nothing the call runs rests on it
        let owned;
        let text = if self.in_spec {
            owned = format!("{text} {IN_SPEC}");
            owned.as_str()
        } else {
            text
        };
        let premise = snapshot(&self.pc);
        let case = Case { goal: c.clone(), pc: premise, draws: None };
        match self.loop_goals.iter_mut().find(|g| g.kind == "pre" && g.text == text) {
            Some(g) => g.cases.push(case),
            None => self.loop_goals.push(Goal {
                kind: "pre".into(),
                expr: GoalExpr::Pre(site.to_string(), Expr::Name(text.to_string(), false)),
                text: text.to_string(),
                cases: vec![case],
            }),
        }
        if !self.in_spec {
            self.pc.borrow_mut().push(c);
        }
    }

    /// In a module with Account-valued states, each Account a call uses as an account (a ledger op, an
    /// Account-keyed state, an Account parameter) that isn't one of this definition's parameters is a call-site
    /// goal: it isn't `none`, which the interpreter refuses to use (DECISIONS.md, Accounts as state values).
    fn accounts_used(&mut self, name: &str, args: &[Expr], vals: &[SV]) {
        if self.defs.account_states.is_empty() {
            return;
        }
        let positions: Vec<usize> = if ["ledger.balance", "ledger.owner", "ledger.debit", "ledger.credit"].contains(&name) {
            vec![0]
        } else if let Some((_, _, "Account")) = state_op(self.defs, name) {
            vec![0]
        } else if let Some(d) = self.defs.defs.get(name).or_else(|| assumed_op(self.defs, name)) {
            d.params.iter().enumerate().filter(|(_, (_, t))| t == "Account").map(|(i, _)| i).collect()
        } else {
            vec![]
        };
        for i in positions {
            let (Some(a), Some(SV::I(v))) = (args.get(i), vals.get(i)) else { continue };
            if matches!(a, Expr::Name(p, false) if self.acct_params.contains(p)) {
                continue;
            }
            let c = zz::ne_int(v, &zz::val(NONE_ID));
            let text = format!("{} is an account, not none (where it is used as one)", code(a));
            self.call_site_goal(name, c, &text);
        }
    }

    /// `p.x` where p holds a record value (a constructor value, its leaves in order): that leaf.
    fn field_of(&self, env: &Env, n: &str) -> Option<SV> {
        let mut at = n.len();
        while let Some(i) = n[..at].rfind('.') {
            if let Some(SV::Ev(r, vals)) = env.get(&n[..i]) {
                if self.defs.records.contains_key(r) {
                    let rest = &n[i + 1..];
                    let k = crate::syntax::record_leaves(&self.defs.records, r).iter().position(|(f, _)| f == rest)?;
                    return vals.get(k).cloned();
                }
            }
            at = i;
        }
        None
    }

    /// A fresh value of a definition's result: an Int, or a record's leaves as one constructor value.
    fn fresh_result(&self, d: &Def, make: &mut dyn FnMut(&str, bool) -> SV) -> Option<SV> {
        let r = d.ret.as_ref()?;
        if !self.defs.records.contains_key(r) {
            return Some(make("", false));
        }
        let leaves = crate::syntax::record_leaves(&self.defs.records, r);
        Some(SV::Ev(r.clone(), leaves.iter().map(|(f, t)| make(f, t == "Text")).collect()))
    }

    fn mutates(&self, name: &str) -> bool {
        matches!(name, "ledger.debit" | "ledger.credit" | "audit.emit" | "clock.tick")
            || channel_op(self.defs, name).is_some()
            || matches!(state_op(self.defs, name), Some((_, "set", _)))
            || (self.defs.defs.get(name).is_some_and(|d| !crate::infer::is_function(self.defs, d)) && assumed_op(self.defs, name).is_none())
    }

    fn scall_read(&mut self, name: &str, a: Vec<SV>, s: &Sym) -> SV {
        match name {
            "ledger.balance" => return SV::I(zz::select(&s.bal, a[0].i())),
            "ledger.owner" => return SV::I(zz::select(&s.own, a[0].i())),
            "ledger.total" => return SV::I(s.total.clone()),
            "clock.now" => return SV::I(s.now.clone()),
            "length" => {
                return SV::I(match &a[0] {
                    SV::Q(xs) => zz::list_len(xs),
                    t => zz::text_len(t.s()),
                })
            }
            "append" => return SV::Q(zz::list_append(a[0].q(), a[1].i())),
            "at" => return SV::I(zz::list_at(a[0].q(), a[1].i())),
            _ => {}
        }
        if let Some(h) = assumed_op(self.defs, name) {
            return self.observe(name, h, &a, s);
        }
        if let Some((x, op, _)) = state_op(self.defs, name) {
            assert_eq!(op, "get", "prover: a state write in an expression");
            if is_text_array(&s.maps[x]) {
                return SV::S(zz::select_text(&s.maps[x], a[0].i()));
            }
            if zz::holds_list(&s.maps[x]) {
                return SV::Q(zz::select_list(&s.maps[x], a[0].i()));
            }
            return SV::I(zz::select(&s.maps[x], a[0].i()));
        }
        if let Some(d) = self.defs.defs.get(name) {
            if crate::infer::is_function(self.defs, d) {
                return self.function_call(d, &a, s);
            }
            panic!("prover: a call to {name} in an expression");
        }
        if name.chars().next().is_some_and(|c| c.is_uppercase()) {
            return SV::Ev(name.to_string(), a);
        }
        if name == "distinct" {
            let xs: Vec<ZInt> = a.iter().map(|x| x.i().clone()).collect();
            return SV::B(zz::distinct(&xs));
        }
        panic!("prover: unknown {name}");
    }

    fn scall_write(&mut self, name: &str, a: Vec<SV>, s: &mut Sym) -> SV {
        if name == "ledger.debit" || name == "ledger.credit" {
            let sign = zz::val(if name == "ledger.debit" { -1 } else { 1 });
            let moved = zz::add(&zz::select(&s.bal, a[0].i()), &zz::mul(&sign, a[1].i()));
            s.bal = zz::store(&s.bal, a[0].i(), &moved);
            s.total = zz::add(&s.total, &zz::mul(&sign, a[1].i()));
            return SV::Unit;
        }
        if name == "clock.tick" {
            // exactly one later instant, as the interpreter does
            s.now = zz::add(&s.now, &zz::val(1));
            return SV::Unit;
        }
        if name == "audit.emit" {
            let ev = match &a[0] {
                SV::Ev(c, args) => Event::Ev(c.clone(), args.clone(), None),
                _ => Event::Opaque("audit.emit".into()),
            };
            s.audit.push(ev);
            return SV::Unit;
        }
        if let Some(ch) = channel_op(self.defs, name) {
            let ev = match &a[0] {
                SV::Ev(c, args) => Event::Ev(c.clone(), args.clone(), None),
                _ => Event::Opaque(name.to_string()),
            };
            s.append(ch, vec![ev]);
            return SV::Unit;
        }
        if let Some((x, "set", _)) = state_op(self.defs, name) {
            let m = match &a[1] {
                SV::S(t) => zz::store_text(&s.maps[x], a[0].i(), t),
                SV::Q(xs) => s.maps[x].store(a[0].i(), xs),
                v => zz::store(&s.maps[x], a[0].i(), v.i()),
            };
            s.maps.insert(x.to_string(), m);
            return SV::Unit;
        }
        let callee = &self.defs.defs[name];
        self.modular_call(callee, &a, s).unwrap_or(SV::Unit)
    }

    /// An assumed handler: any value its spec allows, a fresh one per call; its needs are call-site goals.
    fn observe(&mut self, name: &str, h: &Def, a: &[SV], s: &Sym) -> SV {
        let env: Env = h.params.iter().zip(a).map(|((p, _), v)| (p.clone(), v.clone())).collect();
        for n in &h.needs {
            let cond = need_cond(n);
            let c = self.sev(cond, &env, s, None).b().clone();
            let premise = snapshot(&self.pc);
            self.loop_goals.push(Goal {
                kind: "pre".into(),
                expr: GoalExpr::Pre(name.to_string(), cond.clone()),
                text: format!("{}: need {} (at a call)", name, code(cond)),
                cases: vec![Case { goal: c.clone(), pc: premise, draws: None }],
            });
            self.pc.borrow_mut().push(c);
        }
        let r = if fault("b") { zz::val(0) } else { fresh_int(&name.replace('.', "_")) };
        self.draws.borrow_mut().push((name.to_string(), r.clone()));
        for c in &h.ensures {
            let v = self.sev(c, &env_with(&env, "result", SV::I(r.clone())), s, Some(s)).b().clone();
            self.pc.borrow_mut().push(v);
        }
        self.relied_on.insert(format!("assumed {name}"));
        SV::I(r)
    }

    /// A call, modularly. A definition that returns a value (`-> Int`) gives a fresh `result` its ensures constrain.
    fn modular_call(&mut self, callee: &Def, args: &[SV], s: &mut Sym) -> Option<SV> {
        self.measure_goal(callee, args);
        self.rely_on(&callee.name);
        let mut env: Env = callee.params.iter().zip(args).map(|((p, _), v)| (p.clone(), v.clone())).collect();
        let base = format!("{}_result", callee.name);
        let result = self.fresh_result(callee, &mut |f, text| {
            let name = if f.is_empty() { base.clone() } else { format!("{base}_{}", f.replace('.', "_")) };
            if text {
                SV::S(zz::text_const(&format!("{name}_{}", fresh())))
            } else {
                SV::I(fresh_int(&name))
            }
        });
        if let Some(r) = &result {
            env.insert("result".into(), r.clone());
        }
        for n in &callee.needs {
            let c = self.sev(need_cond(n), &env, s, None).b().clone();
            self.refusal_point(c);
        }
        if !callee.complete && self.refuses_beyond_spec(callee) {
            self.opaque_refusers.borrow_mut().push(callee.name.clone());
        }
        for l in lifted(self.defs, callee) {
            let c = self.sev(&l.cond, &env, s, None).b().clone();
            self.pc.borrow_mut().push(c);
            // a condition rewritten through other callees' specs is exact only if those specs hold
            for r in &l.rests_on {
                self.relied_on.insert(r.clone());
            }
        }
        let pre = s.clone();
        let all_written = writes(self.defs, callee);
        let written = ledger_writes(&all_written, callee);
        if written.contains(ANY_ACCOUNT) {
            s.bal = zz::array(&format!("bal{}", fresh()));
        }
        for p in written.iter().filter(|p| p.as_str() != ANY_ACCOUNT) {
            let v = fresh_int(&format!("bal_{p}_"));
            s.bal = zz::store(&s.bal, env[p.as_str()].i(), &v);
        }
        if !written.is_empty() {
            s.total = fresh_int("total");
        }
        let mut sw: Vec<(String, BTreeSet<String>)> = state_writes(&all_written, callee).into_iter().collect();
        sw.sort();
        for (x, ps) in sw {
            if ps.contains(ANY_ACCOUNT) {
                let fresh_map = zz::array_like(&format!("{}{}", x, fresh()), &s.maps[&x]);
                s.maps.insert(x.clone(), fresh_map);
                continue;
            }
            for p in &ps {
                // the callee's loop wrote [lo, hi) only; Python takes p[1:-1], whatever the closing bracket
                if let Some(range) = p.strip_prefix('[').map(|r| &r[..r.len().saturating_sub(1)]) {
                    let (lo_s, hi_s) = range.split_once(',').expect("a [lo,hi] write");
                    let bound = |b: &str| -> ZInt {
                        let digits = b.trim_start_matches('-');
                        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                            zz::val(b.parse().unwrap())
                        } else {
                            env[b].i().clone()
                        }
                    };
                    let (lo, hi) = (bound(lo_s), bound(hi_s));
                    let old = s.maps[&x].clone();
                    let j = zz::int(&format!("j_{}", fresh()));
                    let new = zz::array_like(&format!("{}_call{}", x, fresh()), &old);
                    s.maps.insert(x.clone(), new.clone());
                    let outside = zz::or(&[zz::lt(&j, &lo), zz::ge(&j, &hi)]);
                    let same = zz::same_at(&new, &old, &j);
                    self.pc.borrow_mut().push(zz::forall(&[&j], &zz::implies(&outside, &same)));
                    let stores: BTreeSet<String> = [x.clone()].into();
                    for (_, g) in self.module_invs_over(&stores, s) {
                        self.pc.borrow_mut().push(g);
                        self.relied_on.insert(format!("invariant on {x}"));
                    }
                    continue;
                }
                let m = if is_text_array(&s.maps[&x]) {
                    let v = zz::text_const(&format!("{x}_{p}_{}", fresh()));
                    zz::store_text(&s.maps[&x], env[p.as_str()].i(), &v)
                } else if zz::holds_list(&s.maps[&x]) {
                    let v = zz::list_const(&format!("{x}_{p}_{}", fresh()));
                    s.maps[&x].store(env[p.as_str()].i(), &v)
                } else {
                    let v = fresh_int(&format!("{x}_{p}_"));
                    zz::store(&s.maps[&x], env[p.as_str()].i(), &v)
                };
                s.maps.insert(x.clone(), m);
            }
        }
        if uses(self.defs, callee).contains("clock.tick") {
            // it may tick any number of times: the time afterwards is later or the same
            let later = fresh_int("now");
            self.pc.borrow_mut().push(zz::ge(&later, &s.now));
            s.now = later;
        }
        // the callee's spec is assumed, not run: it raises no call-site goal (also keeps and emits below)
        self.quiet += 1;
        for c in &callee.ensures {
            let v = self.sev(c, &env, &pre, Some(s)).b().clone();
            self.pc.borrow_mut().push(v);
        }
        for (var, t, cond) in self.invariants() {
            for (p, pt) in &callee.params {
                if pt == t {
                    let e: Env = [(var.clone(), env[p.as_str()].clone())].into_iter().collect();
                    let v = self.sev(cond, &e, s, None).b().clone();
                    self.pc.borrow_mut().push(v);
                }
            }
            // a callee writing a computed key proves the invariant at every key (quantified): so it holds at every
            // key afterwards (this call already relies on the callee)
            if self.invariant_touched(callee, cond) == Some(false) {
                let j = zz::int(&format!("{var}_call_{}", fresh()));
                let v = self.sev(cond, &[(var.clone(), SV::I(j.clone()))].into_iter().collect(), s, None).b().clone();
                self.pc.borrow_mut().push(zz::forall(&[&j], &v));
            }
        }
        for c in &callee.keeps {
            let before = self.sev(c, &env, &pre, None);
            let after = self.sev(c, &env, s, None);
            let v = ev_eq(&before, &after);
            self.pc.borrow_mut().push(v);
        }
        let said = emit_channels(callee);
        let used = uses(self.defs, callee);
        let chans: Vec<String> = std::iter::once("audit".to_string()).chain(self.defs.channels.iter().cloned()).collect();
        for ch in chans {
            if !used.contains(&ch) {
                continue;
            }
            if !said.contains(&ch) {
                s.append(&ch, vec![Event::Opaque(callee.name.clone())]);
                continue;
            }
            for (i, ev) in callee.emits.iter().enumerate() {
                if callee.emit_channel.get(&i).map(String::as_str).unwrap_or("audit") != ch {
                    continue;
                }
                let (ctor, args, cond) = emit_parts(ev);
                let vals: Vec<SV> =
                    args.iter().map(|x| if wild(x) { SV::I(fresh_int("any")) } else { self.sev(x, &env, &pre, None) }).collect();
                let c = cond.map(|c| self.sev(c, &env, &pre, None).b().clone());
                s.append(&ch, vec![Event::Ev(ctor.clone(), vals, c)]);
            }
        }
        self.quiet -= 1;
        result
    }

    /// At a call to `callee` from the body being proved: if the callee can call back into it (recursion), a
    /// `measure` goal: under this path, `0 <= M_callee(args) < M_self(params)`.
    fn measure_goal(&mut self, callee: &Def, args: &[SV]) {
        let Some((me, mine)) = self.measuring.clone() else { return };
        if !reaches(self.defs, &callee.name, &me) {
            return;
        }
        let (Some(m), Some(mine)) = (&callee.decreases, mine) else { return }; // wf rejects a cycle without one
        let env: Env = callee.params.iter().zip(args).map(|((p, _), v)| (p.clone(), v.clone())).collect();
        let none = Sym {
            bal: zz::array("bal"),
            own: zz::array("own"),
            total: zz::int("total"),
            audit: vec![],
            now: zz::int("now"),
            maps: IndexMap::new(),
            out: IndexMap::new(),
        };
        let theirs = self.sev(m, &env, &none, None).i().clone();
        let goal = zz::and(&[zz::ge(&theirs, &zz::val(0)), zz::lt(&theirs, &mine)]);
        let text = format!("decreases {} at the call to {}", code(m), callee.name);
        self.loop_goals.push(Goal {
            kind: "measure".into(),
            expr: GoalExpr::Expr(m.clone()),
            text,
            cases: vec![Case { goal, pc: snapshot(&self.pc), draws: None }],
        });
    }

    /// A call's spec is relied on, except a definition's own (a recursive self-call): its measure goal is what
    /// licenses that, and its own status already says whether it holds.
    fn rely_on(&mut self, name: &str) {
        if self.measuring.as_ref().is_some_and(|(me, _)| me == name) {
            self.self_called = true;
            return;
        }
        self.relied_on.insert(name.to_string());
    }

    /// A pure function, modularly: an uninterpreted function whose result satisfies its ensures.
    fn function_call(&mut self, f: &Def, args: &[SV], s: &Sym) -> SV {
        self.measure_goal(f, args);
        self.rely_on(&f.name);
        // a function that reads state is a function of that state too, so a write in between gives another value;
        // one that reads nothing and takes only Ints keeps its Int-only symbol
        let all_int = args.iter().all(|a| matches!(a, SV::I(_)));
        let reads = !uses(self.defs, f).is_empty();
        let mut ds: Vec<Dynamic> = args.iter().map(SV::dynamic).collect();
        if reads {
            ds.extend(state_args(s));
        }
        // a record result: one uninterpreted function per leaf, `fn_mid.x`
        let r = self
            .fresh_result(f, &mut |leaf, text| {
                let name = if leaf.is_empty() { format!("fn_{}", f.name) } else { format!("fn_{}.{leaf}", f.name) };
                if text {
                    SV::S(zz::apply_fn_dyn_text(&name, &ds))
                } else if !reads && all_int {
                    let xs: Vec<ZInt> = args.iter().map(|a| a.i().clone()).collect();
                    SV::I(zz::apply_fn(&name, &xs))
                } else {
                    SV::I(zz::apply_fn_dyn(&name, &ds))
                }
            })
            .expect("a function returns a value");
        let mut env: Env = f.params.iter().zip(args).map(|((p, _), v)| (p.clone(), v.clone())).collect();
        env.insert("result".into(), r.clone());
        self.quiet += 1;
        for c in &f.ensures {
            let v = self.sev(c, &env, s, Some(s)).b().clone();
            self.pc.borrow_mut().push(v);
        }
        self.quiet -= 1;
        r
    }

    fn refuses_beyond_spec(&self, d: &Def) -> bool {
        flat(&d.body).iter().any(|s| matches!(s, Stmt::Need(..)))
            || callees(self.defs, d).iter().any(|c| !fails(self.defs, &self.defs.defs[c.as_str()]).is_empty())
    }

    fn refusal_point(&mut self, cond: Bool) {
        let premise = self.pc.borrow().clone();
        self.refusals.borrow_mut().push((premise, cond.clone()));
        self.pc.borrow_mut().push(cond);
    }

    /// `ok otherwise`: (goal, reason-if-unknowable).
    fn ok_goal(&self) -> (Option<Bool>, Option<String>) {
        let opq = self.opaque_refusers.borrow();
        if !opq.is_empty() {
            let mut who: Vec<&String> = vec![];
            for o in opq.iter() {
                if !who.contains(&o) {
                    who.push(o);
                }
            }
            let who: Vec<&str> = who.iter().map(|s| s.as_str()).collect();
            return (None, Some(format!("relies on {}, whose spec does not say when it refuses (no `ok otherwise`)", who.join(", "))));
        }
        let reach: Vec<Bool> = self
            .refusals
            .borrow()
            .iter()
            .map(|(pc, c)| {
                let mut xs = pc.clone();
                xs.push(zz::not(c));
                zz::and(&xs)
            })
            .collect();
        (Some(if reach.is_empty() { zz::bval(true) } else { zz::not(&zz::or(&reach)) }), None)
    }

    fn emit_goal(&mut self, d: &Def, env: &Env, pre: &Sym, s: &Sym, ch: &str) -> (Option<Bool>, Option<String>) {
        let events = s.events(ch);
        let emits: Vec<&Emit> =
            d.emits.iter().enumerate().filter(|(i, _)| d.emit_channel.get(i).map(String::as_str).unwrap_or("audit") == ch).map(|(_, e)| e).collect();
        let mut opaque: Vec<String> = vec![];
        for e in &events {
            if let Event::Opaque(w) = e {
                opaque.push(w.clone());
            }
        }
        if opaque.iter().any(|o| o == "a loop") {
            return (None, Some("it emits events inside a loop, which a spec can't describe yet".into()));
        }
        if !opaque.is_empty() {
            let mut who: Vec<String> = vec![];
            for o in opaque {
                if !who.contains(&o) {
                    who.push(o);
                }
            }
            return (None, Some(format!("relies on {}, whose spec does not say what it emits", who.join(", "))));
        }
        let conditional = emits.iter().any(|e| emit_parts(e).2.is_some())
            || events.iter().any(|e| matches!(e, Event::Ev(_, _, Some(_))));
        if conditional {
            return (Some(self.conditional_emit_goal(&emits, env, pre, &events)), None);
        }
        if events.len() != emits.len() {
            return (Some(zz::bval(false)), None);
        }
        let mut conds = vec![zz::bval(true)];
        for (ev, got) in emits.iter().zip(&events) {
            let (ctor, args, _) = emit_parts(ev);
            let Event::Ev(gctor, gargs, _) = got else { unreachable!() };
            if gctor != ctor || gargs.len() != args.len() {
                return (Some(zz::bval(false)), None);
            }
            for (x, v) in args.iter().zip(gargs) {
                if !wild(x) {
                    let want = self.sev(x, env, pre, None);
                    conds.push(ev_eq(&want, v));
                }
            }
        }
        (Some(zz::and(&conds)), None)
    }

    fn conditional_emit_goal(&mut self, emits: &[&Emit], env: &Env, pre: &Sym, events: &[Event]) -> Bool {
        let one = |c: &Bool| zz::ite(c, &zz::val(1), &zz::val(0));
        let want: Vec<(String, Vec<Expr>, Bool)> = emits
            .iter()
            .map(|e| {
                let (ctor, args, cond) = emit_parts(e);
                let c = match cond {
                    Some(c) => self.sev(c, env, pre, None).b().clone(),
                    None => zz::bval(true),
                };
                (ctor.clone(), args.clone(), c)
            })
            .collect();
        let got: Vec<(String, Vec<SV>, Bool)> = events
            .iter()
            .map(|e| match e {
                Event::Ev(c, a, cond) => (c.clone(), a.clone(), cond.clone().unwrap_or_else(|| zz::bval(true))),
                Event::Opaque(_) => unreachable!(),
            })
            .collect();
        let prefix = |cs: Vec<&Bool>| {
            let (mut out, mut acc) = (vec![], zz::val(0));
            for c in cs {
                out.push(acc.clone());
                acc = zz::add(&acc, &one(c));
            }
            (out, acc)
        };
        let (pos_w, n_w) = prefix(want.iter().map(|w| &w.2).collect());
        let (pos_g, n_g) = prefix(got.iter().map(|g| &g.2).collect());
        let mut conds = vec![zz::eq_int(&n_w, &n_g)];
        let wanted: Vec<Vec<Option<SV>>> = want
            .iter()
            .map(|(_, args, _)| args.iter().map(|x| if wild(x) { None } else { Some(self.sev(x, env, pre, None)) }).collect())
            .collect();
        for (i, (ctor, args, cw)) in want.iter().enumerate() {
            for (j, (gctor, vals, cg)) in got.iter().enumerate() {
                let same = if gctor != ctor || vals.len() != args.len() {
                    zz::bval(false)
                } else {
                    let mut parts = vec![zz::bval(true)];
                    for (x, v) in wanted[i].iter().zip(vals) {
                        if let Some(x) = x {
                            parts.push(ev_eq(x, v));
                        }
                    }
                    zz::and(&parts)
                };
                let when = zz::and(&[cw.clone(), cg.clone(), zz::eq_int(&pos_w[i], &pos_g[j])]);
                conds.push(zz::implies(&when, &same));
            }
        }
        zz::and(&conds)
    }

    fn stores_of(&self, cond: &Expr) -> BTreeSet<String> {
        let ns = names(cond);
        let mut out: BTreeSet<String> = ns.iter().filter_map(|n| n.strip_suffix(".get").map(String::from)).collect();
        if ns.contains("ledger.balance") || ns.contains("ledger.total") {
            out.insert("ledger".into());
        }
        out
    }

    /// None if d writes nothing the invariant reads, Some(false) ("any") for a computed key, else Some(true).
    fn invariant_touched(&self, d: &Def, cond: &Expr) -> Option<bool> {
        let reads = names(cond);
        let ws = writes(self.defs, d);
        let mut hit = false;
        let lw = ledger_writes(&ws, d);
        if (reads.contains("ledger.balance") || reads.contains("ledger.total")) && !lw.is_empty() {
            hit = true;
            if lw.contains(ANY_ACCOUNT) {
                return Some(false);
            }
        }
        for (x, ps) in state_writes(&ws, d) {
            if reads.contains(&format!("{x}.get")) {
                hit = true;
                if ps.contains(ANY_ACCOUNT) {
                    return Some(false);
                }
            }
        }
        hit.then_some(true)
    }

    fn bind(&mut self, p: &Path) {
        self.pc = p.pc.clone();
        self.refusals = p.refusals.clone();
        self.opaque_refusers = p.opaque.clone();
        self.draws = p.draws.clone();
    }

    /// Symbolically run statements; an `if` forks the path. Returns the finished paths.
    fn exec(&mut self, stmts: &[Stmt], path: Path) -> Vec<Path> {
        let mut paths = vec![path];
        for s in stmts {
            let mut nxt = vec![];
            for mut p in paths {
                self.bind(&p);
                match s {
                    Stmt::If(c, then, els) => {
                        let c = self.sev(c, &p.env, &p.s, None).b().clone();
                        let a = p.fork(c.clone());
                        nxt.extend(self.exec(then, a));
                        let b = p.fork(zz::not(&c));
                        nxt.extend(self.exec(els, b));
                        continue;
                    }
                    Stmt::For(..) => {
                        self.exec_loop(s, &mut p);
                        self.bind(&p);
                        nxt.push(p);
                        continue;
                    }
                    Stmt::Need(c, _, _) => {
                        let v = self.sev(c, &p.env, &p.s, None).b().clone();
                        p.own.push(v.clone());
                        self.refusal_point(v);
                    }
                    Stmt::Let(x, e) => {
                        let env = p.env.clone();
                        let v = self.sev_mut(e, &env, &mut p.s);
                        p.env.insert(x.clone(), v);
                    }
                    Stmt::Do(e) => {
                        let env = p.env.clone();
                        self.sev_mut(e, &env, &mut p.s);
                    }
                    Stmt::Inv(e) => {
                        let env = p.env.clone();
                        self.sev_mut(e, &env, &mut p.s);
                    }
                }
                nxt.push(p);
            }
            paths = nxt;
        }
        paths
    }

    // ---- loops ---------------------------------------------------------------------------------------------

    /// Whether a loop body may advance the clock (`clock.tick`, directly or in a callee).
    fn may_tick(&self, body: &[Stmt]) -> bool {
        flat(body).iter().any(|s| match call_stmt(s) {
            Some(Expr::App(n, _, _)) | Some(Expr::Name(n, _)) => {
                n == "clock.tick" || self.defs.defs.get(n).is_some_and(|c| uses(self.defs, c).contains("clock.tick"))
            }
            _ => false,
        })
    }

    fn loop_effects(&self, body: &[Stmt]) -> Effects {
        let (mut acc, mut ranged, mut full, mut chans) = (vec![], vec![], BTreeSet::new(), BTreeSet::new());
        for s in flat(body) {
            let (name, args): (&String, &[Expr]) = match call_stmt(s) {
                Some(Expr::App(n, _, a)) => (n, a),
                Some(Expr::Name(n, _)) => (n, &[]),
                _ => continue,
            };
            let st = state_op(self.defs, name);
            if name == "ledger.debit" || name == "ledger.credit" {
                acc.push(Some(args[0].clone()));
            } else if let Some((x, "set", _)) = st {
                match loop_offset(&args[0], &self.loop_var) {
                    None => {
                        full.insert(x.to_string());
                    }
                    Some(off) => {
                        if !ranged.contains(&(x.to_string(), off)) {
                            ranged.push((x.to_string(), off));
                        }
                    }
                }
            } else if name == "audit.emit" {
                chans.insert("audit".to_string());
            } else if let Some(ch) = channel_op(self.defs, name) {
                chans.insert(ch.to_string());
            } else if let Some(callee) = self.defs.defs.get(name.as_str()) {
                let cenv: IndexMap<&str, &Expr> = callee.params.iter().zip(args).map(|((p, _), a)| (p.as_str(), a)).collect();
                for w in writes(self.defs, callee) {
                    match crate::infer::split_write(&w, callee) {
                        None => acc.push(if w != ANY_ACCOUNT { Some(cenv[w.as_str()].clone()) } else { None }),
                        Some((x, _)) => {
                            full.insert(x.to_string());
                        }
                    }
                }
                for u in uses(self.defs, callee) {
                    if u == "audit" || self.defs.channels.contains(&u) {
                        chans.insert(u);
                    }
                }
            }
        }
        let mut shifts: IndexMap<String, BTreeSet<Int>> = IndexMap::new();
        for (x, off) in ranged {
            shifts.entry(x).or_default().insert(off);
        }
        let one: IndexMap<String, Int> = shifts
            .iter()
            .filter(|(x, o)| o.len() == 1 && !full.contains(*x))
            .map(|(x, o)| (x.clone(), *o.iter().next().unwrap()))
            .collect();
        let mut rest = full.clone();
        rest.extend(shifts.keys().filter(|x| !one.contains_key(*x)).cloned());
        (acc, one, rest, chans)
    }

    /// A state after some iterations: written stores fresh, maps written at the loop key equal to S outside
    /// [lo, K). With `empty` (no iteration ran), everything equals S. Returns (state, axioms).
    fn havoc(&mut self, s: &Sym, eff: &Effects, env: &Env, lo: &ZInt, k: &ZInt, empty: Option<Bool>) -> (Sym, Vec<Bool>) {
        let (acc, ranged, full, _) = eff;
        let mut h = s.clone();
        let mut ax = vec![];
        for a in acc {
            match a {
                None => h.bal = zz::array(&format!("bal{}", fresh())),
                Some(a) => {
                    let key = self.sev(a, env, s, None).i().clone();
                    let v = fresh_int("bal_loop_");
                    h.bal = zz::store(&h.bal, &key, &v);
                }
            }
        }
        if !acc.is_empty() {
            h.total = fresh_int("total");
        }
        let mut xs: BTreeSet<String> = ranged.keys().cloned().collect();
        xs.extend(full.iter().cloned());
        for x in &xs {
            let arr = zz::array_like(&format!("{}_loop{}", x, fresh()), &s.maps[x.as_str()]);
            h.maps.insert(x.clone(), arr.clone());
            if let Some(c) = ranged.get(x) {
                let j = zz::int(&format!("j_{}", fresh()));
                let outside = zz::or(&[zz::lt(&j, &zz::add(lo, &zz::val(*c))), zz::ge(&j, &zz::add(k, &zz::val(*c)))]);
                let same = zz::same_at(&arr, &s.maps[x.as_str()], &j);
                ax.push(zz::forall(&[&j], &zz::implies(&outside, &same)));
            }
        }
        if let Some(empty) = empty {
            let mut same = vec![zz::eq_array(&h.bal, &s.bal), zz::eq_int(&h.total, &s.total)];
            for x in &xs {
                same.push(zz::eq_array(&h.maps[x.as_str()], &s.maps[x.as_str()]));
            }
            ax.push(zz::implies(&empty, &zz::and(&same)));
        }
        (h, ax)
    }

    fn inv(&mut self, invs: &[Expr], var: &str, kval: &ZInt, env: &Env, s: &Sym) -> Vec<Bool> {
        let pre = self.call_pre.clone().expect("call_pre");
        let e = env_with(env, var, SV::I(kval.clone()));
        invs.iter().map(|c| self.sev(c, &e, &pre, Some(s)).b().clone()).collect()
    }

    /// Module invariants that read a store the loop writes, for every key.
    fn module_invs_over(&mut self, stores: &BTreeSet<String>, s: &Sym) -> Vec<(String, Bool)> {
        let mut out = vec![];
        for (var, t, cond) in self.invariants() {
            let reads = names(cond);
            let hit = stores.iter().any(|x| reads.contains(&format!("{x}.get")))
                || (stores.contains("ledger") && (reads.contains("ledger.balance") || reads.contains("ledger.total")));
            if hit {
                let j = zz::int(&format!("m_{}", fresh()));
                let e: Env = [(var.clone(), SV::I(j.clone()))].into_iter().collect();
                let body = self.sev(cond, &e, s, None).b().clone();
                out.push((format!("module invariant {}:{} {}", var, t, code(cond)), zz::forall(&[&j], &body)));
            }
        }
        out
    }

    fn exec_loop(&mut self, s: &Stmt, p: &mut Path) {
        let Stmt::For(var, lo_e, hi_e, invs, body) = s else { unreachable!() };
        let lo = self.sev(lo_e, &p.env, &p.s, None).i().clone();
        let hi = self.sev(hi_e, &p.env, &p.s, None).i().clone();
        self.loop_var = var.clone();
        let eff = self.loop_effects(body);
        let ticks = self.may_tick(body);
        let texts: Vec<String> = invs.iter().map(code).collect();
        let mut stores: BTreeSet<String> = eff.1.keys().cloned().collect();
        stores.extend(eff.2.iter().cloned());
        if !eff.0.is_empty() {
            stores.insert("ledger".into());
        }
        // entry
        let env = p.env.clone();
        let mut entry = self.inv(invs, var, &lo, &env, &p.s);
        entry.extend(self.module_invs_over(&stores, &p.s).into_iter().map(|(_, g)| g));
        let mut names: Vec<(String, LoopAst)> = texts.iter().cloned().zip(invs.iter().cloned().map(LoopAst::Inv)).collect();
        names.extend(self.module_invs_over(&stores, &p.s).into_iter().map(|(t, _)| (t.clone(), LoopAst::Module(t))));
        for ((text, ast), g) in names.iter().zip(entry) {
            self.loop_goals.push(Goal {
                kind: "loop".into(),
                expr: GoalExpr::Loop("entry".into(), ast.clone()),
                text: format!("{text}  (holds when the loop starts)"),
                cases: vec![Case { goal: g, pc: snapshot(&p.pc), draws: None }],
            });
        }
        // an arbitrary iteration
        let k = zz::int(&format!("{}_{}", var, fresh()));
        let (mut h, mut ax) = self.havoc(&p.s, &eff, &env, &lo, &k, None);
        if ticks {
            h.now = fresh_int("now_loop");
            ax.push(zz::ge(&h.now, &p.s.now));
        }
        let mut head = self.inv(invs, var, &k, &env, &h);
        head.extend(self.module_invs_over(&stores, &h).into_iter().map(|(_, g)| g));
        let mut pc = p.pc.borrow().clone();
        pc.push(zz::le(&lo, &k));
        pc.push(zz::lt(&k, &hi));
        pc.extend(ax);
        pc.extend(head);
        let mut own = p.own.clone();
        own.push(zz::le(&lo, &k));
        own.push(zz::lt(&k, &hi));
        let it = Path {
            pc: shared(pc),
            s: h,
            env: env_with(&env, var, SV::I(k.clone())),
            refusals: shared(p.refusals.borrow().clone()),
            opaque: shared(p.opaque.borrow().clone()),
            own,
            draws: shared(vec![]),
        };
        let (n_ref, n_opq) = (p.refusals.borrow().len(), p.opaque.borrow().len());
        for q in self.exec(body, it) {
            let k1 = zz::add(&k, &zz::val(1));
            let mut kept = self.inv(invs, var, &k1, &q.env, &q.s);
            kept.extend(self.module_invs_over(&stores, &q.s).into_iter().map(|(_, g)| g));
            for ((text, ast), g) in names.iter().zip(kept) {
                self.loop_goals.push(Goal {
                    kind: "loop".into(),
                    expr: GoalExpr::Loop("kept".into(), ast.clone()),
                    text: format!("{text}  (kept by every iteration)"),
                    cases: vec![Case { goal: g, pc: q.pc.clone(), draws: None }],
                });
            }
            let new_refusals: Vec<_> = q.refusals.borrow()[n_ref..].to_vec();
            p.refusals.borrow_mut().extend(new_refusals);
            let new_opaque: Vec<_> = q.opaque.borrow()[n_opq..].to_vec();
            p.opaque.borrow_mut().extend(new_opaque);
        }
        // after the loop
        let kk = if fault("c") { hi.clone() } else { zz::ite(&zz::ge(&hi, &lo), &hi, &lo) };
        let empty = zz::le(&hi, &lo);
        let (mut a, mut ax) = self.havoc(&p.s, &eff, &env, &lo, &kk, Some(empty.clone()));
        if ticks {
            // later or the same, and the same when the loop doesn't run
            a.now = fresh_int("now_loop");
            ax.push(zz::ge(&a.now, &p.s.now));
            ax.push(zz::implies(&empty, &zz::eq_int(&a.now, &p.s.now)));
        }
        let invk = self.inv(invs, var, &kk, &env, &a);
        let mods: Vec<Bool> = self.module_invs_over(&stores, &a).into_iter().map(|(_, g)| g).collect();
        {
            let mut pcm = p.pc.borrow_mut();
            pcm.extend(ax);
            pcm.extend(invk);
            pcm.extend(mods);
        }
        for ch in &eff.3 {
            a.append(ch, vec![Event::Opaque("a loop".into())]);
        }
        p.s = a;
    }

    // ---- proving -------------------------------------------------------------------------------------------

    /// Prove every guarantee of d. With first_kill (mutation testing), stop at the first confirmed refutation.
    /// Returns (guarantees, relied_on sorted).
    pub fn prove(&mut self, d: &Def, first_kill: bool) -> (Vec<Guarantee>, Vec<String>) {
        match fresh_mode() {
            Fresh::Off => self.prove_here(d, first_kill),
            mode => {
                if mode == Fresh::All {
                    FRESH.with(|c| c.set(0));
                }
                in_fresh_context(|| self.prove_here(d, first_kill))
            }
        }
    }

    fn prove_here(&mut self, d: &Def, first_kill: bool) -> (Vec<Guarantee>, Vec<String>) {
        let (mut out, relied) = self.prove_goals(d, first_kill);
        // a self-call is proven from the definition's own spec (induction), which holds only if the recursion ends:
        // with its measure not proven, what was proven rests on the measure (F5: partial correctness says so)
        if std::mem::take(&mut self.self_called) {
            let open: Vec<String> = out
                .iter()
                .filter(|g| g.kind == "measure" && g.status != Status::Proven)
                .map(|g| format!("termination measure ({})", g.clause))
                .collect();
            if !open.is_empty() {
                for g in out.iter_mut().filter(|g| g.kind != "measure" && matches!(g.status, Status::Proven | Status::Assumed)) {
                    g.status = Status::Conditional;
                    g.blocked_by = Some(open.clone());
                }
            }
        }
        (out, relied)
    }

    fn prove_goals(&mut self, d: &Def, first_kill: bool) -> (Vec<Guarantee>, Vec<String>) {
        self.relied_on = BTreeSet::new();
        self.in_spec = false;
        self.self_called = false;
        self.sums = IndexMap::new();
        let (bal, own, total) = (zz::array("bal"), zz::array("own"), zz::int("total"));
        let maps: IndexMap<String, Array> = self
            .defs
            .states
            .keys()
            .map(|x| {
                let name = format!("map_{x}");
                let arr = if self.defs.list_states.contains(x) {
                    zz::list_array(&name)
                } else {
                    zz::map_array(&name, self.defs.text_states.contains(x))
                };
                (x.clone(), arr)
            })
            .collect();
        let pre = Sym { bal, own, total, audit: vec![], now: zz::int("now"), maps, out: IndexMap::new() };
        let env: Env = d
            .params
            .iter()
            .map(|(p, t)| {
                let v = match t.as_str() {
                    "Text" => SV::S(zz::text_const(p)),
                    "List" => SV::Q(zz::list_const(p)),
                    _ => SV::I(zz::int(p)),
                };
                (p.clone(), v)
            })
            .collect();
        // Int and Account parameters; Text ones are kept apart (a model gives them as strings)
        let params: IndexMap<String, ZInt> =
            env.iter().filter_map(|(p, v)| if let SV::I(x) = v { Some((p.clone(), x.clone())) } else { None }).collect();
        self.text_params =
            env.iter().filter_map(|(p, v)| if let SV::S(x) = v { Some((p.clone(), x.clone())) } else { None }).collect();
        self.list_params =
            env.iter().filter_map(|(p, v)| if let SV::Q(x) = v { Some((p.clone(), x.clone())) } else { None }).collect();
        let mut start = Path {
            pc: shared(vec![]),
            s: pre.clone(),
            env: env.clone(),
            refusals: shared(vec![]),
            opaque: shared(vec![]),
            own: vec![],
            draws: shared(vec![]),
        };
        self.bind(&start);
        self.call_pre = Some(pre.clone());
        self.loop_goals = vec![];
        self.acct_params = d.params.iter().filter(|(_, t)| t == "Account").map(|(p, _)| p.clone()).collect();
        if !self.defs.account_states.is_empty() {
            for p in &self.acct_params {
                self.pc.borrow_mut().push(zz::ne_int(&params[p.as_str()], &zz::val(NONE_ID)));
            }
        }
        let has_loop = flat(&d.body).iter().any(|s| matches!(s, Stmt::For(..)))
            || callees(self.defs, d).iter().any(|c| writes(self.defs, &self.defs.defs[c.as_str()]).iter().any(|w| w.contains('[')));
        if has_loop {
            for (var, _, cond) in self.invariants() {
                let j = zz::int(&format!("pre_{}", fresh()));
                let e: Env = [(var.clone(), SV::I(j.clone()))].into_iter().collect();
                let body = self.sev(cond, &e, &pre, None).b().clone();
                self.pc.borrow_mut().push(zz::forall(&[&j], &body));
                for x in self.stores_of(cond) {
                    self.relied_on.insert(format!("invariant on {x}"));
                }
            }
        }
        for (var, t, cond) in self.invariants() {
            for (p, pt) in &d.params {
                if pt == t {
                    let e: Env = [(var.clone(), env[p.as_str()].clone())].into_iter().collect();
                    let v = self.sev(cond, &e, &pre, None).b().clone();
                    self.pc.borrow_mut().push(v);
                }
            }
        }
        for n in &d.needs {
            let v = self.sev(need_cond(n), &env, &start.s, None).b().clone();
            start.own.push(v.clone());
            self.pc.borrow_mut().push(v);
        }
        // recursive calls in the body must make the measure smaller (DECISIONS.md, recursion)
        let own_measure = d.decreases.as_ref().map(|m| self.sev(m, &env, &pre, None).i().clone());
        self.measuring = Some((d.name.clone(), own_measure));
        let paths = self.exec(&d.body, start);
        self.measuring = None;
        // from here the definition's own spec is evaluated: a call-site goal it raises (an `at` in an ensure) is
        // a goal, but the run has succeeded by then, so it is not assumed (tester, #Y4TVGF6C7A)
        self.in_spec = true;

        let mut goals: Vec<Goal> = vec![];
        for c in &d.keeps {
            let mut cases = vec![];
            for p in &paths {
                self.bind(p);
                let before = self.sev(c, &p.env, &pre, None);
                let after = self.sev(c, &p.env, &p.s, None);
                cases.push(Case { goal: ev_eq(&before, &after), pc: p.pc.clone(), draws: Some(p.draws.clone()) });
            }
            goals.push(Goal { kind: "keep".into(), expr: GoalExpr::Expr(c.clone()), text: code(c), cases });
        }
        for c in &d.ensures {
            let mut cases = vec![];
            for p in &paths {
                self.bind(p);
                let g = self.sev(c, &p.env, &pre, Some(&p.s)).b().clone();
                cases.push(Case { goal: g, pc: p.pc.clone(), draws: Some(p.draws.clone()) });
            }
            let kind = if matches!(c, Expr::Frame(..)) { "keep" } else { "ensure" };
            goals.push(Goal { kind: kind.into(), expr: GoalExpr::Expr(c.clone()), text: code(c), cases });
        }
        let mut unknowable: Vec<Guarantee> = vec![];
        goals.append(&mut std::mem::take(&mut self.loop_goals));
        for (var, t, cond) in self.invariants() {
            let Some(touched) = self.invariant_touched(d, cond) else { continue };
            let text = format!("{}:{} {}", var, t, code(cond));
            let expr = GoalExpr::Invariant(var.clone(), t.clone(), cond.clone());
            if !touched {
                // a computed key (`room * 7 + day`, `item_of.get order`): the invariant at every key, quantified.
                // Before the call it holds everywhere (as it does for every call); after, it must, too.
                let mut cases = vec![];
                for p in &paths {
                    let ja = zz::int(&format!("{var}_post_{}", fresh()));
                    let after = self.sev(cond, &[(var.clone(), SV::I(ja.clone()))].into_iter().collect(), &p.s, None).b().clone();
                    let mut pc = p.pc.borrow().clone();
                    // every module invariant holds at every key before the call, not only this one
                    for (v2, _, c2) in self.invariants() {
                        let jb = zz::int(&format!("{v2}_pre_{}", fresh()));
                        let before = self.sev(c2, &[(v2.clone(), SV::I(jb.clone()))].into_iter().collect(), &pre, None).b().clone();
                        pc.push(zz::forall(&[&jb], &before));
                    }
                    cases.push(Case { goal: zz::forall(&[&ja], &after), pc: shared(pc), draws: None });
                }
                goals.push(Goal { kind: "invariant".into(), expr, text, cases });
                continue;
            }
            let mut cases = vec![];
            for p in &paths {
                let mut parts = vec![zz::bval(true)];
                for (q, qt) in &d.params {
                    if qt == t {
                        let e: Env = [(var.clone(), p.env[q.as_str()].clone())].into_iter().collect();
                        parts.push(self.sev(cond, &e, &p.s, None).b().clone());
                    }
                }
                cases.push(Case { goal: zz::and(&parts), pc: p.pc.clone(), draws: None });
            }
            goals.push(Goal { kind: "invariant".into(), expr, text, cases });
        }
        if d.complete {
            self.refusals = shared(paths.iter().flat_map(|p| p.refusals.borrow().clone()).collect());
            self.opaque_refusers = shared(paths.iter().flat_map(|p| p.opaque.borrow().clone()).collect());
            match self.ok_goal() {
                (_, Some(why)) => unknowable.push(Guarantee { reason: Some(why), ..guarantee("ok", GoalExpr::None, "otherwise", Status::Unknown) }),
                (Some(g), None) => goals.insert(
                    0,
                    Goal { kind: "ok".into(), expr: GoalExpr::None, text: "otherwise".into(), cases: vec![Case { goal: g, pc: shared(vec![]), draws: None }] },
                ),
                (None, None) => unreachable!(),
            }
        }
        for ch in emit_channels(d) {
            let emits: Vec<Emit> = d
                .emits
                .iter()
                .enumerate()
                .filter(|(i, _)| d.emit_channel.get(i).map(String::as_str).unwrap_or("audit") == ch)
                .map(|(_, e)| e.clone())
                .collect();
            let mut cases = vec![];
            let mut why_unknown = None;
            for p in &paths {
                self.bind(p);
                let (g, w) = self.emit_goal(d, &p.env, &pre, &p.s, &ch);
                if why_unknown.is_none() {
                    why_unknown = w;
                }
                cases.push((g, p.pc.clone(), p.draws.clone()));
            }
            let text = (if ch == "audit" { String::new() } else { format!("{ch} ") })
                + &emits.iter().map(emit_code).collect::<Vec<_>>().join(", then ");
            if let Some(why) = why_unknown {
                unknowable.push(Guarantee {
                    reason: Some(why),
                    channel: Some(ch.clone()),
                    ..guarantee("emit", GoalExpr::Emits(emits), &text, Status::Unknown)
                });
            } else {
                let expr = if ch == "audit" { GoalExpr::Emits(emits) } else { GoalExpr::ChannelEmits(ch.clone(), emits) };
                let cases = cases.into_iter().map(|(g, pc, dr)| Case { goal: g.expect("a goal"), pc, draws: Some(dr) }).collect();
                goals.push(Goal { kind: "emit".into(), expr, text, cases });
            }
        }
        if let Some(mut vacuous) = self.vacuity(&paths) {
            if goals.iter().any(|g| g.kind == "loop") {
                vacuous = "no input gets past a loop: its invariants can't all hold when it ends (see the loop's own goals)".into();
            }
            let mut out = vec![];
            for g in &goals {
                if g.kind == "loop" || g.kind == "pre" {
                    out.push(self.solve(d, g, &params, &pre));
                } else {
                    out.push(Guarantee { reason: Some(vacuous.clone()), ..guarantee(&g.kind, g.expr.clone(), &g.text, Status::Vacuous) });
                }
            }
            for u in unknowable {
                out.push(Guarantee { status: Status::Vacuous, reason: Some(vacuous.clone()), ..u });
            }
            return (out, self.relied_on.iter().cloned().collect());
        }
        let mut results = vec![];
        self.unknowns_left = first_kill.then_some(MUTANT_UNKNOWNS);
        for g in &goals {
            results.push(self.solve(d, g, &params, &pre));
            if first_kill && results.last().unwrap().status == Status::Refuted {
                self.unknowns_left = None;
                return (results, self.relied_on.iter().cloned().collect());
            }
            if self.unknowns_left == Some(0) {
                break; // a mutant whose goals keep coming back unknown: the prover doesn't kill it
            }
        }
        self.unknowns_left = None;
        results.extend(unknowable);
        (results, self.relied_on.iter().cloned().collect())
    }

    fn solve(&mut self, d: &Def, goal: &Goal, params: &IndexMap<String, ZInt>, pre: &Sym) -> Guarantee {
        let mut unknown = false;
        for case in &goal.cases {
            self.replay_draws = case.draws.as_ref().map(|d| d.borrow().clone()).unwrap_or_default();
            let assumed = case.pc.borrow().clone();
            let solver = zz::solver();
            for a in &assumed {
                solver.assert(a);
            }
            solver.assert(&zz::not(&case.goal));
            match zz::check(&solver) {
                zz::Check::Unsat => continue,
                zz::Check::Unknown => {
                    unknown = true;
                    if let Some(n) = self.unknowns_left.as_mut() {
                        *n = n.saturating_sub(1);
                        if *n == 0 {
                            break;
                        }
                    }
                    continue;
                }
                zz::Check::Sat => {}
            }
            let nice = zz::solver();
            for a in &assumed {
                nice.assert(a);
            }
            nice.assert(&zz::not(&case.goal));
            nice.assert(&zz::ge_n(&pre.now, 0));
            nice.assert(&zz::le_n(&pre.now, 1000));
            for (p, t) in &d.params {
                if t == "List" {
                    // a short list reads best
                    nice.assert(&zz::le_n(&zz::list_len(&self.list_params[p.as_str()]), 4));
                    continue;
                }
                if t == "Text" {
                    continue;
                }
                let v = &params[p.as_str()];
                if t == "Int" {
                    nice.assert(&zz::ge_n(v, 0));
                    nice.assert(&zz::le_n(v, 1000));
                } else {
                    nice.assert(&zz::ge_n(v, 0));
                    nice.assert(&zz::le_n(v, 3));
                }
                if t == "Account" {
                    nice.assert(&zz::ge_n(&zz::select(&pre.bal, v), 0));
                    nice.assert(&zz::le_n(&zz::select(&pre.bal, v), 1000));
                    nice.assert(&zz::ge_n(&zz::select(&pre.own, v), 0));
                    nice.assert(&zz::le_n(&zz::select(&pre.own, v), 1));
                }
                for (x, arr) in &pre.maps {
                    if self.defs.states[x.as_str()] == *t && !is_text_array(arr) && !zz::holds_list(arr) {
                        nice.assert(&zz::ge_n(&zz::select(arr, v), 0));
                        nice.assert(&zz::le_n(&zz::select(arr, v), 1000));
                    }
                }
            }
            let cx = self.find_cex(d, &nice, &solver, params, pre, &goal.kind);
            let status = if cx.confirmed_by_interpreter { Status::Refuted } else { Status::Unconfirmed };
            return Guarantee { counterexample: Some(cx), ..guarantee(&goal.kind, goal.expr.clone(), &goal.text, status) };
        }
        guarantee(&goal.kind, goal.expr.clone(), &goal.text, if unknown { Status::Unknown } else { Status::Proven })
    }

    /// If no input can succeed, every guarantee holds trivially. Say so, and say whose fault.
    fn vacuity(&self, paths: &[Path]) -> Option<String> {
        let unsat = |cs: &[Bool]| {
            let s = zz::solver();
            for c in cs {
                s.assert(c);
            }
            zz::check(&s) == zz::Check::Unsat
        };
        if !paths.iter().all(|p| unsat(&p.pc.borrow())) {
            return None;
        }
        if paths.iter().all(|p| unsat(&p.own)) {
            return Some("no input passes its own needs, so the call never succeeds".into());
        }
        let who: Vec<&str> = self.relied_on.iter().map(String::as_str).collect();
        Some(format!("no input succeeds given the specs of {} (they contradict each other or its needs)", who.join(", ")))
    }

    fn find_cex(&mut self, d: &Def, nice: &z3::Solver, solver: &z3::Solver, params: &IndexMap<String, ZInt>, pre: &Sym, kind: &str) -> Counterexample {
        if zz::check(nice) != zz::Check::Sat {
            let m = solver.get_model().expect("a model");
            return self.render_cex(d, &m, params, pre, kind);
        }
        let cx = self.render_cex(d, &nice.get_model().expect("a model"), params, pre, kind);
        if cx.confirmed_by_interpreter {
            return cx;
        }
        let assertions = nice.get_assertions();
        if assertions.iter().any(|a| zz::sexpr(a).contains("forall")) {
            let small = zz::solver();
            for a in &assertions {
                small.assert(a);
            }
            for (p, t) in &d.params {
                if t == "Int" {
                    small.assert(&zz::ge_n(&params[p.as_str()], 0));
                    small.assert(&zz::le_n(&params[p.as_str()], 5));
                }
            }
            if zz::check(&small) == zz::Check::Sat {
                let cx2 = self.render_cex(d, &small.get_model().expect("a model"), params, pre, kind);
                if cx2.confirmed_by_interpreter {
                    return cx2;
                }
            }
            let mut m = nice.get_model().expect("a model");
            for _ in 0..8 {
                let differ: Vec<Bool> =
                    d.params
                        .iter()
                        .filter(|(_, t)| t != "Text")
                        .map(|(p, _)| zz::ne_int(&params[p.as_str()], &m.eval(&params[p.as_str()], true).expect("a value")))
                        .collect();
                nice.assert(&zz::or(&differ));
                if zz::check(nice) != zz::Check::Sat {
                    break;
                }
                m = nice.get_model().expect("a model");
                let cx2 = self.render_cex(d, &m, params, pre, kind);
                if cx2.confirmed_by_interpreter {
                    return cx2;
                }
            }
            return cx;
        }
        let opt = zz::optimize();
        for a in &assertions {
            opt.assert(a);
        }
        for (p, t) in &d.params {
            if t == "Account" {
                let b = zz::select(&pre.bal, &params[p.as_str()]);
                if kind == "ok" {
                    opt.minimize(&b);
                } else {
                    opt.maximize(&b);
                }
            } else if t != "Text" && t != "List" {
                opt.minimize(&params[p.as_str()]);
            }
        }
        if zz::check_opt(&opt) == zz::Check::Sat {
            let cx2 = self.render_cex(d, &opt.get_model().expect("a model"), params, pre, kind);
            if cx2.confirmed_by_interpreter {
                return cx2;
            }
        }
        cx
    }

    /// Turn a Z3 model into an `eg` line, then replay it in the interpreter to confirm.
    fn render_cex(&mut self, d: &Def, m: &Model, params: &IndexMap<String, ZInt>, pre: &Sym, kind: &str) -> Counterexample {
        let cx = self.render_cex_once(d, m, params, pre, kind, &[]);
        if cx.confirmed_by_interpreter {
            return cx;
        }
        // the model's array may not list a key computed from a parameter (`lo - 1`): read it at likely keys, the
        // parameters' values give or take 2 and the definition's integer literals, and try once more
        let mut keys: Vec<i128> = vec![];
        for (p, t) in &d.params {
            if t == "Int" {
                if let Some(v) = zz::eval_long(m, &params[p.as_str()]) {
                    keys.extend((-2..=2).map(|o| v + o));
                }
            }
        }
        keys.extend(literals(d));
        keys.sort();
        keys.dedup();
        let cx2 = self.render_cex_once(d, m, params, pre, kind, &keys);
        if cx2.confirmed_by_interpreter {
            cx2
        } else {
            cx
        }
    }

    fn render_cex_once(&mut self, d: &Def, m: &Model, params: &IndexMap<String, ZInt>, pre: &Sym, kind: &str, probe: &[i128])
        -> Counterexample {
        let long = |x: &ZInt| zz::eval_long(m, x).expect("a numeral in the model");
        let (mut letters, mut owners): (IndexMap<i128, String>, IndexMap<i128, String>) = (IndexMap::new(), IndexMap::new());
        let mut world: IndexMap<String, (Int, String)> = IndexMap::new();
        let mut args: Vec<Value> = vec![];
        for (p, t) in &d.params {
            if t == "Text" {
                args.push(Value::Str(zz::text_value(m, &self.text_params[p.as_str()]).unwrap_or_default()));
                continue;
            }
            if t == "List" {
                let xs = zz::list_value(m, &self.list_params[p.as_str()], 50).unwrap_or_default();
                args.push(Value::Ctor(crate::run::LIST.into(), xs.into_iter().map(Value::Int).collect()));
                continue;
            }
            let v = long(&params[p.as_str()]);
            if t == "Account" {
                if !letters.contains_key(&v) {
                    let letter = "ABCDEFGH".chars().nth(letters.len()).expect("at most 8 accounts").to_string();
                    letters.insert(v, letter.clone());
                    let bal = long(&zz::select(&pre.bal, &params[p.as_str()]));
                    let o = long(&zz::select(&pre.own, &params[p.as_str()]));
                    let next = format!("u{}", owners.len() + 1);
                    owners.entry(o).or_insert(next);
                    world.insert(letter, (bal, owners[&o].clone()));
                }
                args.push(Value::Str(letters[&v].clone()));
            } else {
                args.push(Value::Int(v));
            }
        }
        let now = long(&pre.now);
        let mut maps: IndexMap<String, IndexMap<Key, Int>> = IndexMap::new();
        let mut keys: Vec<(String, Key)> = vec![];
        let mut texts: crate::syntax::Texts = IndexMap::new();
        let mut text_keys: Vec<(String, Key)> = vec![];
        let mut list_maps: IndexMap<String, IndexMap<Key, Vec<Int>>> = IndexMap::new();
        // an invariant may read states the definition never touches (`hold <= cap + 100`): show them all then
        let shown = if kind == "invariant" { self.defs.states.clone() } else { relevant_states(self.defs, d) };
        for (x, arr) in &pre.maps {
            // an Account-valued state: every entry shown (0 is an account there, an unset entry is none)
            let acct_state = self.defs.account_states.contains(x);
            if !shown.contains_key(x) {
                continue;
            }
            if zz::holds_list(arr) {
                // a List state: its entries at the parameters' keys
                let st = &self.defs.states[x.as_str()];
                for (p, t) in d.params.iter().filter(|(_, t)| t == st) {
                    let zk = long(&params[p.as_str()]);
                    let key = if t == "Account" { Key::Str(letters[&zk].clone()) } else { Key::Int(zk) };
                    if let Some(xs) = zz::list_value(m, &zz::select_list(arr, &params[p.as_str()]), 50).filter(|xs| !xs.is_empty()) {
                        list_maps.entry(x.clone()).or_default().insert(key, xs.into_iter().map(|v| v as Int).collect());
                    }
                }
                continue;
            }
            if is_text_array(arr) {
                // a Text state: its entries at the parameters' keys
                let st = &self.defs.states[x.as_str()];
                for (p, t) in d.params.iter().filter(|(_, t)| t == st) {
                    let zk = long(&params[p.as_str()]);
                    let key = if t == "Account" { Key::Str(letters[&zk].clone()) } else { Key::Int(zk) };
                    if text_keys.contains(&(x.clone(), key.clone())) {
                        continue;
                    }
                    text_keys.push((x.clone(), key.clone()));
                    let val = zz::text_value(m, &zz::select_text(arr, &params[p.as_str()])).unwrap_or_default();
                    if !val.is_empty() {
                        texts.entry(x.clone()).or_default().insert(key, val);
                    }
                }
                // and every entry the model gives, as for Int maps (a key computed from a parameter: `k + 1`)
                if st == "Int" {
                    let whole = m.eval(arr, true).expect("an array value");
                    for (zk, val) in zz::text_array_entries(m, &whole, 50) {
                        if !text_keys.contains(&(x.clone(), Key::Int(zk))) && zk.abs() <= 1_000_000 {
                            text_keys.push((x.clone(), Key::Int(zk)));
                            if !val.is_empty() {
                                texts.entry(x.clone()).or_default().insert(Key::Int(zk), val);
                            }
                        }
                    }
                    for zk in probe {
                        if !text_keys.contains(&(x.clone(), Key::Int(*zk))) {
                            text_keys.push((x.clone(), Key::Int(*zk)));
                            let val = zz::text_value(m, &zz::select_text(arr, &zz::val(*zk))).unwrap_or_default();
                            if !val.is_empty() {
                                texts.entry(x.clone()).or_default().insert(Key::Int(*zk), val);
                            }
                        }
                    }
                }
                continue;
            }
            let st = &self.defs.states[x.as_str()];
            for (p, t) in &d.params {
                if st != t {
                    continue;
                }
                let zk = long(&params[p.as_str()]);
                let key = if t == "Account" { Key::Str(letters[&zk].clone()) } else { Key::Int(zk) };
                if keys.contains(&(x.clone(), key.clone())) {
                    continue;
                }
                keys.push((x.clone(), key.clone()));
                let val = long(&zz::select(arr, &params[p.as_str()]));
                if val != 0 || acct_state {
                    maps.entry(x.clone()).or_default().insert(key, val);
                }
            }
            if st == "Int" {
                let whole = m.eval(arr, true).expect("an array value");
                for (zk, val) in zz::array_entries(m, &whole, 50) {
                    if !keys.contains(&(x.clone(), Key::Int(zk))) && zk.abs() <= 1_000_000 {
                        keys.push((x.clone(), Key::Int(zk)));
                        if val != 0 || acct_state {
                            maps.entry(x.clone()).or_default().insert(Key::Int(zk), val);
                        }
                    }
                }
                for zk in probe {
                    if !keys.contains(&(x.clone(), Key::Int(*zk))) {
                        keys.push((x.clone(), Key::Int(*zk)));
                        if let Some(val) = zz::eval_long(m, &zz::select(arr, &zz::val(*zk))).filter(|v| *v != 0 || acct_state) {
                            maps.entry(x.clone()).or_default().insert(Key::Int(*zk), val);
                        }
                    }
                }
            }
        }
        // an Account-valued state's entries are accounts (or none): named like parameters, added to the world
        for x in &self.defs.account_states {
            let Some(kv) = maps.shift_remove(x) else { continue };
            for (k, v) in kv {
                let name = if v as i128 == NONE_ID {
                    crate::run::NONE.to_string()
                } else {
                    let v = v as i128;
                    if !letters.contains_key(&v) {
                        let letter = "ABCDEFGH".chars().nth(letters.len()).expect("at most 8 accounts").to_string();
                        letters.insert(v, letter.clone());
                        let bal = long(&zz::select(&pre.bal, &zz::val(v)));
                        let o = long(&zz::select(&pre.own, &zz::val(v)));
                        let next = format!("u{}", owners.len() + 1);
                        owners.entry(o).or_insert(next);
                        world.insert(letter, (bal, owners[&o].clone()));
                    }
                    letters[&v].clone()
                };
                texts.entry(x.clone()).or_default().insert(k, name);
            }
        }
        let key_text = |k: &Key| match k {
            Key::Int(n) => n.to_string(),
            Key::Str(s) => s.clone(),
        };
        let mut world_src = world.iter().map(|(a, (b, o))| format!("{a}:{b}@{o}")).collect::<Vec<_>>().join(" ");
        if now != 0 {
            world_src += &format!(" now:{now}");
        }
        for (x, kv) in &maps {
            for (k, v) in kv {
                world_src += &format!(" {}.{}:{}", x, key_text(k), v);
            }
        }
        for (x, kv) in &texts {
            let acct = self.defs.account_states.contains(x);
            for (k, v) in kv {
                // an account's name (or none) as it is; a Text quoted
                let shown = if acct { v.clone() } else { crate::syntax::quote(v) };
                world_src += &format!(" {}.{}:{}", x, key_text(k), shown);
            }
        }
        // a Text argument is quoted (an account name isn't): the line must parse back to the same call
        let list_text = |xs: &[Int]| format!("[{}]", xs.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "));
        for (x, kv) in &list_maps {
            for (k, xs) in kv {
                world_src += &format!(" {}.{}:{}", x, key_text(k), list_text(xs));
            }
        }
        let shown_args = args.iter().zip(&d.params).map(|(a, (_, t))| match a {
            Value::Str(v) if t == "Text" => crate::syntax::quote(v),
            Value::Ctor(_, xs) if t == "List" => list_text(&xs.iter().filter_map(Value::num).collect::<Vec<_>>()),
            a => a.str(),
        });
        let call = std::iter::once(d.name.clone()).chain(shown_args).collect::<Vec<_>>().join(" ");
        let mut w = World::from_example(&world, now, &maps).with_texts(&texts).with_lists(&list_maps);
        let draws: VecDeque<Int> = self.replay_draws.iter().map(|(_, t)| long(t)).collect();
        let drawn: Vec<Int> = draws.iter().copied().collect();
        let queue = Rc::new(RefCell::new(draws));
        let q2 = queue.clone();
        let provider: crate::run::Provider = Box::new(move |name: &str, a: &[Value]| {
            if let Some(v) = q2.borrow_mut().pop_front() {
                return Ok(Value::Int(v));
            }
            if name != "rand.below" {
                return Err(ProvideError::Other("KeyError".into(), format!("'{name}'")));
            }
            let n = a.first().and_then(Value::num).ok_or_else(|| ProvideError::Other("TypeError".into(), "bad bound".into()))?;
            if n <= 0 {
                return Err(ProvideError::Other("ValueError".into(), "empty range for randrange()".into()));
            }
            let mut r = crate::pyrandom::Random::from_int(0);
            Ok(Value::Int(r.randrange(0, n as i64) as Int))
        });
        let mut interp = Interp::with_provider(self.defs, Some(provider), 0);
        interp.budget = Some(REPLAY_BUDGET);
        let before = w.clone();
        let outcome = match interp.run(&d.name, &args, &mut w) {
            Ok(o) => o,
            Err(RunError::Kern(e)) => {
                let eg = format!("eg {world_src} | {call} -> error: {e}");
                self.witness_draws.insert(eg.clone(), drawn.clone());
                DRAWS.with(|d| d.borrow_mut().insert(eg.clone(), drawn));
                return Counterexample {
                    eg,
                    emitted: "nothing".into(),
                    // a measure that doesn't decrease: the replay recursing past the depth limit is the evidence
                    confirmed_by_interpreter: (kind == "pre"
                        && (e.contains("need of") || e.contains(crate::run::NONE_USED) || e.contains(crate::run::AT_OUTSIDE)))
                        || (kind == "measure" && e.starts_with(crate::run::TOO_DEEP)),
                };
            }
            Err(e) => panic!("counterexample replay of {}: {e:?}", d.name),
        };
        let mut after = w;
        let mut after_src = world.keys().map(|a| format!("{}:{}", a, after.bal[a.as_str()])).collect::<Vec<_>>().join(" ");
        for (x, k) in &keys {
            let got = after.maps.get(x).and_then(|kv| kv.get(k)).copied().unwrap_or(0);
            let was = maps.get(x).and_then(|kv| kv.get(k)).copied().unwrap_or(0);
            if got != was {
                after_src += &format!(" {}.{}:{}", x, key_text(k), got);
            }
        }
        for (x, k) in &text_keys {
            let got = after.text_get(x, k);
            let was = texts.get(x).and_then(|kv| kv.get(k)).cloned().unwrap_or_default();
            if got != was {
                after_src += &format!(" {}.{}:{}", x, key_text(k), crate::syntax::quote(&got));
            }
        }
        let out_src = outcome.text();
        let mut outcome = outcome;
        let violated = match kind {
            "loop" => {
                interp.check_loops = true;
                interp.budget = Some(REPLAY_BUDGET);
                let mut again = before.clone();
                match interp.run(&d.name, &args, &mut again) {
                    Ok(o) => {
                        outcome = o;
                        after = again;
                        let mut before2 = before.clone();
                        !interp.loop_violations.is_empty()
                            || (outcome == Outcome::Ok
                                && !interp.broken_invariants(&mut after, None).expect("invariants").is_empty()
                                && interp.broken_invariants(&mut before2, None).expect("invariants").is_empty())
                    }
                    Err(RunError::Kern(_)) => false,
                    Err(e) => panic!("loop replay of {}: {e:?}", d.name),
                }
            }
            "ok" => interp.unstated_refusal(d, &args, &before, &outcome).expect("unstated_refusal"),
            "invariant" => {
                let mut before2 = before.clone();
                outcome == Outcome::Ok
                    && !interp.broken_invariants(&mut after, None).expect("invariants").is_empty()
                    && interp.broken_invariants(&mut before2, None).expect("invariants").is_empty()
            }
            _ => {
                if outcome == Outcome::Ok {
                    let mut before2 = before.clone();
                    !interp.holds(d, &args, &mut before2, &mut after).expect("holds").is_empty()
                } else {
                    false
                }
            }
        };
        let emitted = if outcome == Outcome::Ok {
            after
                .audit
                .iter()
                .map(|ev| match ev {
                    Value::Ctor(c, a) => std::iter::once(c.clone()).chain(a.iter().map(Value::str)).collect::<Vec<_>>().join(" "),
                    other => other.str(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            String::new()
        };
        let eg = format!("eg {world_src} | {call} -> {out_src} | {after_src}");
        self.witness_draws.insert(eg.clone(), drawn.clone());
        DRAWS.with(|d| d.borrow_mut().insert(eg.clone(), drawn));
        Counterexample {
            eg,
            emitted: if emitted.is_empty() { "nothing".into() } else { emitted },
            confirmed_by_interpreter: violated,
        }
    }
}

fn guarantee(kind: &str, expr: GoalExpr, clause: &str, status: Status) -> Guarantee {
    Guarantee {
        kind: kind.to_string(),
        expr,
        clause: clause.to_string(),
        status,
        counterexample: None,
        blocked_by: None,
        assuming: None,
        via_broken_spec: None,
        reason: None,
        vacuity: None,
        tested: None,
        channel: None,
    }
}

/// report.py's handler_sat: can an assumed handler's spec be met? ∀ args. needs → ∃ result. ensures.
/// "satisfiable", "unsatisfiable for P = V, …", "unsatisfiable" (by sampling) or "not checked".
pub fn handler_sat(defs: &Module, h: &Def) -> String {
    if fresh_mode() == Fresh::Off {
        handler_sat_here(defs, h)
    } else {
        in_fresh_context(|| handler_sat_here(defs, h))
    }
}

fn handler_sat_here(defs: &Module, h: &Def) -> String {
    let mut pv = Prover::new(defs);
    // Python passes no state (None): a handler's spec can only read its parameters and `result`
    let none = Sym {
        bal: zz::array("bal"),
        own: zz::array("own"),
        total: zz::int("total"),
        audit: vec![],
        now: zz::int("now"),
        maps: IndexMap::new(),
        out: IndexMap::new(),
    };
    let env: Env = h.params.iter().map(|(p, _)| (p.clone(), SV::I(zz::int(p)))).collect();
    let r = zz::int("result");
    let mut needs = vec![zz::bval(true)];
    for n in &h.needs {
        needs.push(pv.sev(need_cond(n), &env, &none, None).b().clone());
    }
    let needs = zz::and(&needs);
    let mut ens = vec![zz::bval(true)];
    let renv = env_with(&env, "result", SV::I(r.clone()));
    for c in &h.ensures {
        ens.push(pv.sev(c, &renv, &none, None).b().clone());
    }
    let ens = zz::and(&ens);
    let s = zz::solver();
    s.assert(&needs);
    s.assert(&zz::forall(&[&r], &zz::not(&ens)));
    match zz::check(&s) {
        zz::Check::Unsat => return "satisfiable".into(),
        zz::Check::Sat => {
            let m = s.get_model().expect("a model");
            let parts: Vec<String> = h
                .params
                .iter()
                .map(|(p, _)| format!("{} = {}", p, zz::eval_long(&m, env[p.as_str()].i()).expect("a numeral")))
                .collect();
            return format!("unsatisfiable for {}", parts.join(", "));
        }
        zz::Check::Unknown => {}
    }
    for v in [0, 1, 2, 5, 100] {
        let s = zz::solver();
        for (p, _) in &h.params {
            s.assert(&zz::eq_n(env[p.as_str()].i(), v));
        }
        s.assert(&needs);
        s.assert(&zz::not(&zz::exists(&[&r], &ens)));
        if zz::check(&s) == zz::Check::Sat {
            return "unsatisfiable".into();
        }
    }
    "not checked".into()
}
