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
use z3::ast::{Array, Bool, Int as ZInt};
use z3::Model;

use crate::infer::{callees, fails, ledger_writes, lifted, names, relevant_states, state_writes, uses, writes, ANY_ACCOUNT};
use crate::report::{Counterexample, GoalExpr, Guarantee, LoopAst, Status};
use crate::run::{assumed_op, channel_op, emit_channels, state_op, Interp, Outcome, ProvideError, RunError, Value, World};
use crate::syntax::{code, emit_code, flat, Def, Emit, Expr, Key, Module, Need, Stmt};
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

/// Whether each proof gets a fresh Z3 context (KERN_Z3_FRESH): `off` (one context per thread, Python's
/// behaviour), `ctx` (a fresh context per prove call) or `all` (that, and the fresh-name counter restarts too, so
/// a proof's result can't depend on anything proved before it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fresh {
    Off,
    Ctx,
    All,
}

pub fn fresh_mode() -> Fresh {
    match std::env::var("KERN_Z3_FRESH").as_deref() {
        Ok("ctx") => Fresh::Ctx,
        Ok("all") => Fresh::All,
        _ => Fresh::Off,
    }
}

/// Run f with a brand-new Z3 context as the thread's context, then put the old one back. The z3 crate only offers
/// this for Send + Sync closures; f runs synchronously on this thread, so asserting it is sound here.
fn in_fresh_context<R>(f: impl FnOnce() -> R) -> R {
    struct Unchecked<T>(T);
    unsafe impl<T> Send for Unchecked<T> {}
    unsafe impl<T> Sync for Unchecked<T> {}
    let ctx = unsafe {
        let cfg = z3_sys::Z3_mk_config().expect("Z3 config");
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
            Expr::All(v, a, b, body) => {
                let j = zz::int(&format!("{}_{}", v, fresh()));
                let lo = self.sev(a, env, cur, post);
                let hi = self.sev(b, env, cur, post);
                let inner = self.sev(body, &env_with(env, v, SV::I(j.clone())), cur, post);
                let range = zz::and(&[zz::le(lo.i(), &j), zz::lt(&j, hi.i())]);
                SV::B(zz::forall(&[&j], &zz::implies(&range, inner.b())))
            }
            Expr::Bin(op, l, r) => {
                let l = self.sev(l, env, cur, post);
                let r = self.sev(r, env, cur, post);
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
                let s = if *primed { post.expect("a primed read needs a post-state") } else { cur };
                self.scall_read(n, vec![], s)
            }
            Expr::App(name, primed, args) => {
                let vals: Vec<SV> = args.iter().map(|a| self.sev(a, env, cur, post)).collect();
                let s = if *primed { post.expect("a primed read needs a post-state") } else { cur };
                self.scall_read(name, vals, s)
            }
        }
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
        self.scall_write(name, vals, s)
    }

    fn mutates(&self, name: &str) -> bool {
        matches!(name, "ledger.debit" | "ledger.credit" | "audit.emit")
            || channel_op(self.defs, name).is_some()
            || matches!(state_op(self.defs, name), Some((_, "set", _)))
            || (self.defs.defs.get(name).is_some_and(|d| d.ret.is_none()) && assumed_op(self.defs, name).is_none())
    }

    fn scall_read(&mut self, name: &str, a: Vec<SV>, s: &Sym) -> SV {
        match name {
            "ledger.balance" => return SV::I(zz::select(&s.bal, a[0].i())),
            "ledger.owner" => return SV::I(zz::select(&s.own, a[0].i())),
            "ledger.total" => return SV::I(s.total.clone()),
            "clock.now" => return SV::I(s.now.clone()),
            _ => {}
        }
        if let Some(h) = assumed_op(self.defs, name) {
            return self.observe(name, h, &a, s);
        }
        if let Some((x, op, _)) = state_op(self.defs, name) {
            assert_eq!(op, "get", "prover: a state write in an expression");
            return SV::I(zz::select(&s.maps[x], a[0].i()));
        }
        if let Some(d) = self.defs.defs.get(name) {
            if d.ret.is_some() {
                return SV::I(self.function_call(d, &a, s));
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
            let m = zz::store(&s.maps[x], a[0].i(), a[1].i());
            s.maps.insert(x.to_string(), m);
            return SV::Unit;
        }
        let callee = &self.defs.defs[name];
        self.modular_call(callee, &a, s);
        SV::Unit
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
        let r = fresh_int(&name.replace('.', "_"));
        self.draws.borrow_mut().push((name.to_string(), r.clone()));
        for c in &h.ensures {
            let v = self.sev(c, &env_with(&env, "result", SV::I(r.clone())), s, Some(s)).b().clone();
            self.pc.borrow_mut().push(v);
        }
        self.relied_on.insert(format!("assumed {name}"));
        SV::I(r)
    }

    fn modular_call(&mut self, callee: &Def, args: &[SV], s: &mut Sym) {
        self.relied_on.insert(callee.name.clone());
        let env: Env = callee.params.iter().zip(args).map(|((p, _), v)| (p.clone(), v.clone())).collect();
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
        }
        let pre = s.clone();
        let all_written = writes(self.defs, callee);
        let written = ledger_writes(&all_written);
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
        let mut sw: Vec<(String, BTreeSet<String>)> = state_writes(&all_written).into_iter().collect();
        sw.sort();
        for (x, ps) in sw {
            if ps.contains(ANY_ACCOUNT) {
                s.maps.insert(x.clone(), zz::array(&format!("{}{}", x, fresh())));
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
                    let new = zz::array(&format!("{}_call{}", x, fresh()));
                    s.maps.insert(x.clone(), new.clone());
                    let outside = zz::or(&[zz::lt(&j, &lo), zz::ge(&j, &hi)]);
                    let same = zz::eq_int(&zz::select(&new, &j), &zz::select(&old, &j));
                    self.pc.borrow_mut().push(zz::forall(&[&j], &zz::implies(&outside, &same)));
                    let stores: BTreeSet<String> = [x.clone()].into();
                    for (_, g) in self.module_invs_over(&stores, s) {
                        self.pc.borrow_mut().push(g);
                        self.relied_on.insert(format!("invariant on {x}"));
                    }
                    continue;
                }
                let v = fresh_int(&format!("{x}_{p}_"));
                let m = zz::store(&s.maps[&x], env[p.as_str()].i(), &v);
                s.maps.insert(x.clone(), m);
            }
        }
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
    }

    /// A pure function, modularly: an uninterpreted function whose result satisfies its ensures.
    fn function_call(&mut self, f: &Def, args: &[SV], s: &Sym) -> ZInt {
        self.relied_on.insert(f.name.clone());
        let xs: Vec<ZInt> = args.iter().map(|a| a.i().clone()).collect();
        let r = zz::apply_fn(&format!("fn_{}", f.name), &xs);
        let mut env: Env = f.params.iter().zip(args).map(|((p, _), v)| (p.clone(), v.clone())).collect();
        env.insert("result".into(), SV::I(r.clone()));
        for c in &f.ensures {
            let v = self.sev(c, &env, s, Some(s)).b().clone();
            self.pc.borrow_mut().push(v);
        }
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
        let lw = ledger_writes(&ws);
        if (reads.contains("ledger.balance") || reads.contains("ledger.total")) && !lw.is_empty() {
            hit = true;
            if lw.contains(ANY_ACCOUNT) {
                return Some(false);
            }
        }
        for (x, ps) in state_writes(&ws) {
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

    fn loop_effects(&self, body: &[Stmt]) -> Effects {
        let (mut acc, mut ranged, mut full, mut chans) = (vec![], vec![], BTreeSet::new(), BTreeSet::new());
        for s in flat(body) {
            let Stmt::Do(Expr::App(name, _, args)) = s else { continue };
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
                    match w.rsplit_once('.') {
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
            let arr = zz::array(&format!("{}_loop{}", x, fresh()));
            h.maps.insert(x.clone(), arr.clone());
            if let Some(c) = ranged.get(x) {
                let j = zz::int(&format!("j_{}", fresh()));
                let outside = zz::or(&[zz::lt(&j, &zz::add(lo, &zz::val(*c))), zz::ge(&j, &zz::add(k, &zz::val(*c)))]);
                let same = zz::eq_int(&zz::select(&arr, &j), &zz::select(&s.maps[x.as_str()], &j));
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
        let (h, ax) = self.havoc(&p.s, &eff, &env, &lo, &k, None);
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
        let kk = zz::ite(&zz::ge(&hi, &lo), &hi, &lo);
        let empty = zz::le(&hi, &lo);
        let (mut a, ax) = self.havoc(&p.s, &eff, &env, &lo, &kk, Some(empty));
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
        self.relied_on = BTreeSet::new();
        let (bal, own, total) = (zz::array("bal"), zz::array("own"), zz::int("total"));
        let maps: IndexMap<String, Array> = self.defs.states.keys().map(|x| (x.clone(), zz::array(&format!("map_{x}")))).collect();
        let pre = Sym { bal, own, total, audit: vec![], now: zz::int("now"), maps, out: IndexMap::new() };
        let env: Env = d.params.iter().map(|(p, _)| (p.clone(), SV::I(zz::int(p)))).collect();
        let params: IndexMap<String, ZInt> = env.iter().map(|(p, v)| (p.clone(), v.i().clone())).collect();
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
        let paths = self.exec(&d.body, start);

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
            goals.push(Goal { kind: "ensure".into(), expr: GoalExpr::Expr(c.clone()), text: code(c), cases });
        }
        let mut unknowable: Vec<Guarantee> = vec![];
        goals.append(&mut std::mem::take(&mut self.loop_goals));
        for (var, t, cond) in self.invariants() {
            let Some(touched) = self.invariant_touched(d, cond) else { continue };
            let text = format!("{}:{} {}", var, t, code(cond));
            let expr = GoalExpr::Invariant(var.clone(), t.clone(), cond.clone());
            if !touched {
                unknowable.push(Guarantee {
                    reason: Some("it writes a computed key, so the invariant can't be checked key by key".into()),
                    ..guarantee("invariant", expr, &text, Status::Unknown)
                });
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
        for g in &goals {
            results.push(self.solve(d, g, &params, &pre));
            if first_kill && results.last().unwrap().status == Status::Refuted {
                return (results, self.relied_on.iter().cloned().collect());
            }
        }
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
                    if self.defs.states[x.as_str()] == *t {
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
                    d.params.iter().map(|(p, _)| zz::ne_int(&params[p.as_str()], &m.eval(&params[p.as_str()], true).expect("a value"))).collect();
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
            } else {
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
        let long = |x: &ZInt| zz::eval_long(m, x).expect("a numeral in the model");
        let (mut letters, mut owners): (IndexMap<i128, String>, IndexMap<i128, String>) = (IndexMap::new(), IndexMap::new());
        let mut world: IndexMap<String, (Int, String)> = IndexMap::new();
        let mut args: Vec<Value> = vec![];
        for (p, t) in &d.params {
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
        let shown = relevant_states(self.defs, d);
        for (x, arr) in &pre.maps {
            if !shown.contains_key(x) {
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
                if val != 0 {
                    maps.entry(x.clone()).or_default().insert(key, val);
                }
            }
            if st == "Int" {
                let whole = m.eval(arr, true).expect("an array value");
                for (zk, val) in zz::array_entries(m, &whole, 50) {
                    if !keys.contains(&(x.clone(), Key::Int(zk))) && zk.abs() <= 1_000_000 {
                        keys.push((x.clone(), Key::Int(zk)));
                        if val != 0 {
                            maps.entry(x.clone()).or_default().insert(Key::Int(zk), val);
                        }
                    }
                }
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
        let call = std::iter::once(d.name.clone()).chain(args.iter().map(Value::str)).collect::<Vec<_>>().join(" ");
        let mut w = World::from_example(&world, now, &maps);
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
                    confirmed_by_interpreter: kind == "pre" && e.contains("need of"),
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
