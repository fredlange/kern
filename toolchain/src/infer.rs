//! Infers what the agent never has to write (port of kern/infer.py): the effect set (`uses`), the failure set
//! (`fails`), the frame (`writes`) and the body-level refusal conditions (`lifted`).
//!
//! Python returns sets for `uses` and `writes`; here they are BTreeSets, so any iteration is in sorted order (every
//! Python caller that prints one sorts it first; FZ finding 6).

use std::collections::BTreeSet;

use indexmap::{IndexMap, IndexSet};

use crate::run::{effect_of, is_handler, state_op};
use crate::syntax::{flat, negate, subst, Def, Emit, Expr, Module, Need, Stmt};

pub const WRITERS: [&str; 2] = ["ledger.debit", "ledger.credit"];
pub const ANY_ACCOUNT: &str = "*";
const STATE_READS: [&str; 2] = ["ledger.balance", "ledger.total"]; // owners are never written

/// Every sub-expression, pre-order (as Python's `_walk`).
pub fn walk(e: &Expr) -> Vec<&Expr> {
    let mut out = vec![];
    fn go<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
        out.push(e);
        match e {
            Expr::All(_, a, b, body) => {
                go(a, out);
                go(b, out);
                go(body, out);
            }
            Expr::Bin(_, l, r) => {
                go(l, out);
                go(r, out);
            }
            Expr::App(_, _, args) => args.iter().for_each(|a| go(a, out)),
            _ => {}
        }
    }
    go(e, &mut out);
    out
}

/// The statements a definition runs: its spec needs (as need statements), then its body.
pub fn need_stmts(d: &Def) -> Vec<Stmt> {
    d.needs
        .iter()
        .filter_map(|n| match n {
            Need::Need(c, k, a) => Some(Stmt::Need(c.clone(), k.clone(), a.clone())),
            Need::As(..) => None,
        })
        .collect()
}

fn stmt_exprs(s: &Stmt) -> Vec<&Expr> {
    match s {
        Stmt::For(_, lo, hi, _, _) => vec![lo, hi],
        Stmt::Inv(_) => vec![],
        Stmt::Let(_, e) => vec![e],
        Stmt::Need(c, _, args) => std::iter::once(c).chain(args.iter()).collect(),
        Stmt::Do(e) | Stmt::If(e, _, _) => vec![e],
    }
}

/// Names a definition's needs and body mention: application heads and plain names, in order.
pub fn called_names(d: &Def) -> Vec<String> {
    let needs = need_stmts(d);
    let mut out = vec![];
    for s in flat(&needs).into_iter().chain(flat(&d.body)) {
        for e in stmt_exprs(s) {
            for n in walk(e) {
                match n {
                    Expr::App(h, _, _) => out.push(h.clone()),
                    Expr::Name(x, _) => out.push(x.clone()),
                    _ => {}
                }
            }
        }
    }
    out
}

fn with_seen(seen: &BTreeSet<String>, name: &str) -> BTreeSet<String> {
    let mut s = seen.clone();
    s.insert(name.to_string());
    s
}

pub fn uses(defs: &Module, d: &Def) -> BTreeSet<String> {
    uses_seen(defs, d, &BTreeSet::new())
}

fn uses_seen(defs: &Module, d: &Def, seen: &BTreeSet<String>) -> BTreeSet<String> {
    let seen = with_seen(seen, &d.name);
    let mut eff = BTreeSet::new();
    for n in called_names(d) {
        if is_handler(defs, &n) {
            eff.insert(effect_of(defs, &n));
        } else if let Some(c) = defs.defs.get(&n) {
            if !seen.contains(&n) {
                eff.extend(uses_seen(defs, c, &seen));
            }
        }
    }
    // writing a store implies reading it
    let writes: Vec<String> = eff.iter().filter(|e| e.ends_with(".write")).cloned().collect();
    for e in writes {
        eff.remove(&format!("{}read", &e[..e.len() - "write".len()]));
    }
    eff
}

fn arg_type(e: &Expr, d: &Def) -> String {
    if let Expr::Name(n, _) = e {
        if let Some((_, t)) = d.params.iter().find(|(p, _)| p == n) {
            return t.clone();
        }
    }
    "Int".into()
}

pub type Fails = IndexMap<String, Vec<String>>;

pub fn fails(defs: &Module, d: &Def) -> Fails {
    fails_seen(defs, d, &BTreeSet::new())
}

fn fails_seen(defs: &Module, d: &Def, seen: &BTreeSet<String>) -> Fails {
    let seen = with_seen(seen, &d.name);
    let mut out = Fails::new();
    let needs = need_stmts(d);
    for s in flat(&needs).into_iter().chain(flat(&d.body)) {
        if let Stmt::Need(_, ctor, args) = s {
            out.entry(ctor.clone()).or_insert_with(|| args.iter().map(|a| arg_type(a, d)).collect());
        }
    }
    for n in called_names(d) {
        if let Some(c) = defs.defs.get(&n) {
            if !seen.contains(&n) {
                for (k, v) in fails_seen(defs, c, &seen) {
                    out.entry(k).or_insert(v);
                }
            }
        }
    }
    out
}

type Lets = IndexMap<String, Expr>;

/// Follow `x = y` let chains (Python loops forever on `x = x`; this stops after the chain repeats).
fn resolve<'a>(mut e: &'a Expr, lets: &'a Lets) -> &'a Expr {
    let mut steps = 0;
    while let Expr::Name(n, _) = e {
        match lets.get(n) {
            Some(v) if steps <= lets.len() => {
                e = v;
                steps += 1;
            }
            _ => break,
        }
    }
    e
}

/// Which parameter an Account expression denotes, or ANY_ACCOUNT if it cannot be named.
fn account_param(e: &Expr, d: &Def, lets: &Lets) -> String {
    match resolve(e, lets) {
        Expr::Name(n, _) if d.params.iter().any(|(p, _)| p == n) => n.clone(),
        _ => ANY_ACCOUNT.into(),
    }
}

pub fn writes(defs: &Module, d: &Def) -> BTreeSet<String> {
    writes_seen(defs, d, &BTreeSet::new())
}

/// The balances a call can change, as parameter names (ANY_ACCOUNT = could be any account), and state entries
/// `X.param`, `X.*`, or `X.[lo,hi)` for a map written only at the key of a loop `for k from lo to hi` whose bounds
/// are parameters or numbers (the loop's inferred frame).
fn writes_seen(defs: &Module, d: &Def, seen: &BTreeSet<String>) -> BTreeSet<String> {
    let seen = with_seen(seen, &d.name);
    let mut lets = Lets::new();
    let mut out = BTreeSet::new();
    type Loops = IndexMap<String, (Option<String>, Option<String>)>;

    fn bound(e: &Expr, d: &Def, lets: &Lets) -> Option<String> {
        match e {
            Expr::Int(n) => Some(n.to_string()),
            Expr::Name(n, _) if d.params.iter().any(|(p, _)| p == n) && !lets.contains_key(n) => Some(n.clone()),
            _ => None,
        }
    }

    fn key(e: &Expr, loops: &Loops, d: &Def, lets: &Lets) -> String {
        if let Expr::Name(n, _) = resolve(e, lets) {
            if let Some((lo, hi)) = loops.get(n) {
                return match (lo, hi) {
                    (Some(lo), Some(hi)) => format!("[{},{})", lo, hi),
                    _ => ANY_ACCOUNT.into(),
                };
            }
        }
        account_param(e, d, lets)
    }

    #[allow(clippy::too_many_arguments)]
    fn go(stmts: &[Stmt], loops: &Loops, defs: &Module, d: &Def, seen: &BTreeSet<String>, lets: &mut Lets,
          out: &mut BTreeSet<String>) {
        for s in stmts {
            match s {
                Stmt::Let(x, e) => {
                    lets.insert(x.clone(), e.clone());
                }
                Stmt::If(_, t, o) => {
                    go(t, loops, defs, d, seen, lets, out);
                    go(o, loops, defs, d, seen, lets, out);
                }
                Stmt::For(k, lo, hi, _, body) => {
                    let mut l2 = loops.clone();
                    l2.insert(k.clone(), (bound(lo, d, lets), bound(hi, d, lets)));
                    go(body, &l2, defs, d, seen, lets, out);
                }
                Stmt::Do(Expr::App(name, _, args)) => {
                    let st = state_op(defs, name);
                    if WRITERS.contains(&name.as_str()) {
                        out.insert(account_param(&args[0], d, lets));
                    } else if let Some((x, "set", _)) = st {
                        out.insert(format!("{}.{}", x, key(&args[0], loops, d, lets)));
                    } else if let Some(callee) = defs.defs.get(name) {
                        if seen.contains(name) {
                            continue;
                        }
                        let idx: IndexMap<&str, usize> =
                            callee.params.iter().enumerate().map(|(i, (p, _))| (p.as_str(), i)).collect();
                        for w in writes_seen(defs, callee, seen) {
                            let (x, dot, p) = match w.rfind('.') {
                                Some(i) => (&w[..i], true, &w[i + 1..]),
                                None => ("", false, w.as_str()),
                            };
                            if p.starts_with('[') {
                                // a callee's loop range, mapped to our args
                                let ends: Vec<Option<String>> = p[1..p.len() - 1]
                                    .split(',')
                                    .map(|b| {
                                        let digits = b.trim_start_matches('-');
                                        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                                            Some(b.to_string())
                                        } else {
                                            bound(&args[idx[b]], d, lets)
                                        }
                                    })
                                    .collect();
                                match (&ends[0], &ends[1]) {
                                    (Some(a), Some(b)) => out.insert(format!("{}.[{},{})", x, a, b)),
                                    _ => out.insert(format!("{}.{}", x, ANY_ACCOUNT)),
                                };
                                continue;
                            }
                            let mapped =
                                if p == ANY_ACCOUNT { ANY_ACCOUNT.to_string() } else { account_param(&args[idx[p]], d, lets) };
                            out.insert(if dot { format!("{}.{}", x, mapped) } else { mapped });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    go(&d.body, &Loops::new(), defs, d, &seen, &mut lets, &mut out);
    out
}

pub fn ledger_writes(ws: &BTreeSet<String>) -> BTreeSet<String> {
    ws.iter().filter(|w| !w.contains('.')).cloned().collect()
}

/// {state: {param or ANY_ACCOUNT}} from a writes() set.
pub fn state_writes(ws: &BTreeSet<String>) -> IndexMap<String, BTreeSet<String>> {
    let mut out: IndexMap<String, BTreeSet<String>> = IndexMap::new();
    for w in ws {
        if let Some(i) = w.rfind('.') {
            out.entry(w[..i].to_string()).or_default().insert(w[i + 1..].to_string());
        }
    }
    out
}

pub fn fmt_writes(ws: &BTreeSet<String>, d: &Def) -> String {
    let led = ledger_writes(ws);
    let mut parts: Vec<String> = if led.contains(ANY_ACCOUNT) {
        vec!["any account".into()]
    } else {
        d.params.iter().filter(|(p, _)| led.contains(p)).map(|(p, _)| p.clone()).collect()
    };
    let mut sw: Vec<_> = state_writes(ws).into_iter().collect();
    sw.sort();
    for (x, ps) in sw {
        if ps.contains(ANY_ACCOUNT) {
            parts.push(format!("{}.*", x));
            continue;
        }
        parts.extend(d.params.iter().filter(|(q, _)| ps.contains(q)).map(|(q, _)| format!("{}.{}", x, q)));
        parts.extend(ps.iter().filter(|p| p.starts_with('[')).map(|p| format!("{}.{}", x, p)));
    }
    parts.join(" ")
}

/// Names (plain and application heads) in an expression.
pub fn names(e: &Expr) -> IndexSet<String> {
    let mut out = IndexSet::new();
    for n in walk(e) {
        match n {
            Expr::Name(x, _) | Expr::App(x, _, _) => {
                out.insert(x.clone());
            }
            _ => {}
        }
    }
    out
}

fn reads_state(e: &Expr) -> bool {
    names(e).iter().any(|n| STATE_READS.contains(&n.as_str()) || n.ends_with(".get"))
}

/// A refusal condition lifted to the caller's pre-state: (cond, ctor, payload or None, origin).
#[derive(Clone, Debug, PartialEq)]
pub struct Lifted {
    pub cond: Expr,
    pub ctor: String,
    pub payload: Option<Vec<Expr>>,
    /// "" for d's own body, else the callee it came from
    pub origin: String,
}

/// Conditions the call needs in order to succeed, stated in the caller's pre-state.
///
/// Spec needs are already in the interface; this adds the *body-level* ones (own `need`s and, transitively,
/// callees' needs), so callers and the prover can see when a call refuses. A condition is lifted only when it can be
/// stated in the pre-state: it comes before any ledger write, or it does not read balances at all.
pub fn lifted(defs: &Module, d: &Def) -> Vec<Lifted> {
    lifted_seen(defs, d, &BTreeSet::new())
}

struct Lifter<'a> {
    defs: &'a Module,
    seen: BTreeSet<String>,
    out: Vec<Lifted>,
    /// name -> (expr in pre-state terms, usable)
    lets: IndexMap<String, (Expr, bool)>,
    clean: bool,
}

impl<'a> Lifter<'a> {
    fn pre_state(&self, e: &Expr) -> Option<Expr> {
        if names(e).iter().any(|n| matches!(self.lets.get(n), Some((_, false)))) {
            return None;
        }
        let m: IndexMap<String, Expr> = self.lets.iter().map(|(n, (v, _))| (n.clone(), v.clone())).collect();
        let e2 = subst(e, &m);
        if self.clean || !reads_state(&e2) {
            Some(e2)
        } else {
            None
        }
    }

    fn add(&mut self, cond: &Expr, ctor: &str, payload: &[Expr], origin: &str, guards: &[Option<Expr>]) {
        let Some(mut c) = self.pre_state(cond) else { return };
        if guards.iter().any(|g| g.is_none()) {
            return;
        }
        for g in guards.iter().rev() {
            // inside `if G`: the call refuses when G and not cond, so callers may assume `not G or cond`
            let Some(ng) = negate(g.as_ref().unwrap()) else { return };
            c = Expr::Bin("or", Box::new(ng), Box::new(c));
        }
        let p: Vec<Option<Expr>> = payload.iter().map(|a| self.pre_state(a)).collect();
        let payload = if p.iter().any(|x| x.is_none()) { None } else { Some(p.into_iter().map(Option::unwrap).collect()) };
        if self.out.iter().all(|o| !(o.cond == c && o.ctor == ctor)) {
            self.out.push(Lifted { cond: c, ctor: ctor.to_string(), payload, origin: origin.to_string() });
        }
    }

    fn walk(&mut self, stmts: &[Stmt], guards: &[Option<Expr>]) {
        for s in stmts {
            match s {
                Stmt::Let(x, e) => {
                    let v = self.pre_state(e);
                    let usable = v.is_some();
                    self.lets.insert(x.clone(), (v.unwrap_or_else(|| e.clone()), usable));
                }
                Stmt::Need(c, ctor, args) => self.add(c, ctor, args, "", guards),
                Stmt::Inv(_) => continue,
                Stmt::For(_, _, _, _, body) => {
                    // a loop's refusals depend on earlier iterations: never lifted; any effect ends lifting
                    if flat(body).iter().any(|x| matches!(x, Stmt::Do(_))) {
                        self.clean = false;
                    }
                }
                Stmt::If(c, t, o) => {
                    let g = self.pre_state(c);
                    let ng = g.as_ref().and_then(negate);
                    let before = self.clean;
                    let mut g1 = guards.to_vec();
                    g1.push(g);
                    self.walk(t, &g1);
                    let after_then = self.clean;
                    self.clean = before;
                    let mut g2 = guards.to_vec();
                    g2.push(ng);
                    self.walk(o, &g2);
                    self.clean = self.clean && after_then;
                }
                Stmt::Do(Expr::App(name, _, args)) => {
                    if let Some(callee) = self.defs.defs.get(name) {
                        if self.seen.contains(name) {
                            continue;
                        }
                        let m: IndexMap<String, Expr> =
                            callee.params.iter().map(|(p, _)| p.clone()).zip(args.iter().cloned()).collect();
                        for n in &callee.needs {
                            if let Need::Need(c, ctor, a) = n {
                                let a: Vec<Expr> = a.iter().map(|x| subst(x, &m)).collect();
                                self.add(&subst(c, &m), ctor, &a, name, guards);
                            }
                        }
                        for l in lifted_seen(self.defs, callee, &self.seen) {
                            let a: Vec<Expr> = l.payload.map(|p| p.iter().map(|x| subst(x, &m)).collect()).unwrap_or_default();
                            let origin = if l.origin.is_empty() { name.clone() } else { l.origin };
                            self.add(&subst(&l.cond, &m), &l.ctor, &a, &origin, guards);
                        }
                        if !writes(self.defs, callee).is_empty() {
                            self.clean = false;
                        }
                    } else if WRITERS.contains(&name.as_str())
                        || matches!(state_op(self.defs, name), Some((_, "set", _)))
                    {
                        self.clean = false;
                    }
                }
                Stmt::Do(_) => {}
            }
        }
    }
}

fn lifted_seen(defs: &Module, d: &Def, seen: &BTreeSet<String>) -> Vec<Lifted> {
    let mut l = Lifter { defs, seen: with_seen(seen, &d.name), out: vec![], lets: IndexMap::new(), clean: true };
    l.walk(&d.body, &[]);
    l.out
}

fn emit_exprs(d: &Def) -> Vec<&Expr> {
    let mut out = vec![];
    for ev in &d.emits {
        let (Emit::Ev(_, args, cond) | Emit::As(_, args, _, cond)) = ev;
        out.extend(args.iter());
        out.extend(cond.iter());
    }
    out
}

/// Declared state that matters for checking d: what its body (and callees) read or write, and what its spec
/// clauses read (a spec may describe state the body forgot to touch).
pub fn relevant_states(defs: &Module, d: &Def) -> IndexMap<String, String> {
    let touched = uses(defs, d);
    let exprs: Vec<&Expr> = d.keeps.iter().chain(d.ensures.iter()).chain(emit_exprs(d)).collect();
    let mut named = BTreeSet::new();
    for e in exprs {
        for n in walk(e) {
            if let Expr::App(h, _, _) = n {
                if let Some(x) = h.strip_suffix(".get") {
                    named.insert(x.to_string());
                }
            }
        }
    }
    defs.states
        .iter()
        .filter(|(x, _)| {
            touched.contains(&format!("{}.read", x)) || touched.contains(&format!("{}.write", x)) || named.contains(*x)
        })
        .map(|(x, t)| (x.clone(), t.clone()))
        .collect()
}

/// Functions (-> Int) a definition's spec clauses name: the reader needs their specs too.
pub fn spec_functions(defs: &Module, d: &Def) -> Vec<String> {
    let mut exprs: Vec<&Expr> = vec![];
    for n in &d.needs {
        if let Need::Need(c, _, _) = n {
            exprs.push(c);
        }
    }
    for n in &d.needs {
        if let Need::Need(_, _, a) = n {
            exprs.extend(a.iter());
        }
    }
    exprs.extend(d.keeps.iter());
    exprs.extend(d.ensures.iter());
    exprs.extend(emit_exprs(d));
    let mut out = vec![];
    for e in exprs {
        for n in walk(e) {
            if let Expr::App(h, _, _) = n {
                if defs.defs.get(h).map_or(false, |c| c.ret.is_some()) {
                    out.push(h.clone());
                }
            }
        }
    }
    out
}

pub fn callees(defs: &Module, d: &Def) -> Vec<String> {
    let mut seen = IndexSet::new();
    for n in called_names(d).into_iter().chain(spec_functions(defs, d)) {
        seen.insert(n);
    }
    seen.into_iter().filter(|n| defs.defs.contains_key(n) && *n != d.name).collect()
}

pub fn fmt_uses(eff: &BTreeSet<String>) -> String {
    let order = ["ledger.write", "ledger.read", "clock", "audit"];
    let mut v: Vec<&String> = eff.iter().collect();
    v.sort_by_key(|x| (order.iter().position(|o| o == x).unwrap_or(99), (*x).clone()));
    v.into_iter().cloned().collect::<Vec<_>>().join(" ")
}

pub fn fmt_fails(fs: &Fails) -> String {
    fs.iter()
        .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{}({})", k, v.join(",")) })
        .collect::<Vec<_>>()
        .join(" ")
}
