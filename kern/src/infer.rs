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
/// stated exactly in the pre-state: it doesn't read state, or every write before it is a direct one (`ledger.debit`,
/// `ledger.credit`, `x.set`) whose after-values can be written in pre-state terms. A read after such a write is
/// rewritten through the writes, splitting on aliasing (`ledger.balance x` after `ledger.debit a n` is
/// `ledger.balance x - n` if x = a, else unchanged). A callee that writes, a loop with effects, or an `if` whose
/// branches write still ends lifting for state-reading conditions: the after-state is no longer exact there.
pub fn lifted(defs: &Module, d: &Def) -> Vec<Lifted> {
    lifted_seen(defs, d, &BTreeSet::new())
}

/// A direct write, in pre-state terms.
#[derive(Clone, Debug, PartialEq)]
enum Write {
    /// `ledger.debit a n` (op "-") or `ledger.credit a n` (op "+")
    Bal(Expr, &'static str, Expr),
    /// `x.set k v`
    Map(String, Expr, Expr),
}

/// The most case combinations a rewritten condition may have; beyond it the condition isn't lifted.
const MAX_CASES: usize = 8;

struct Lifter<'a> {
    defs: &'a Module,
    seen: BTreeSet<String>,
    out: Vec<Lifted>,
    /// name -> (expr in pre-state terms, usable)
    lets: IndexMap<String, (Expr, bool)>,
    /// the direct writes so far, in order; None once the after-state can't be stated exactly
    log: Option<Vec<Write>>,
    /// pairs that differ on every path reaching this point: `a != b` refusal points already passed
    distinct: Vec<(Expr, Expr)>,
}

fn bin(op: &'static str, l: Expr, r: Expr) -> Expr {
    Expr::Bin(op, Box::new(l), Box::new(r))
}

/// Replace whole sub-expressions (simultaneously; replacements are not searched again).
fn replace_terms(e: &Expr, m: &[(Expr, Expr)]) -> Expr {
    if let Some((_, v)) = m.iter().find(|(t, _)| t == e) {
        return v.clone();
    }
    match e {
        Expr::Bin(op, l, r) => bin(op, replace_terms(l, m), replace_terms(r, m)),
        Expr::App(h, p, args) => Expr::App(h.clone(), *p, args.iter().map(|a| replace_terms(a, m)).collect()),
        Expr::All(j, a, b, body) => {
            Expr::All(j.clone(), Box::new(replace_terms(a, m)), Box::new(replace_terms(b, m)), Box::new(replace_terms(body, m)))
        }
        _ => e.clone(),
    }
}

fn is_total(e: &Expr) -> bool {
    matches!(e, Expr::Name(n, false) if n == "ledger.total") || matches!(e, Expr::App(n, false, a) if n == "ledger.total" && a.is_empty())
}

impl<'a> Lifter<'a> {
    fn same(a: &Expr, b: &Expr) -> bool {
        a == b
    }

    fn differ(&self, a: &Expr, b: &Expr) -> bool {
        matches!((a, b), (Expr::Int(x), Expr::Int(y)) if x != y) || self.distinct.iter().any(|(l, r)| (l == a && r == b) || (l == b && r == a))
    }

    /// `e` with lets substituted, or None if it uses a let that has no pre-state value.
    fn with_lets(&self, e: &Expr) -> Option<Expr> {
        if names(e).iter().any(|n| matches!(self.lets.get(n), Some((_, false)))) {
            return None;
        }
        let m: IndexMap<String, Expr> = self.lets.iter().map(|(n, (v, _))| (n.clone(), v.clone())).collect();
        Some(subst(e, &m))
    }

    /// The state reads in `e` and, for each, its possible values after the logged writes: (conditions, value).
    /// None if a read can't be resolved (its key reads written state, or the after-state is unknown).
    fn read_cases(&self, e: &Expr) -> Option<Vec<(Expr, Vec<(Vec<Expr>, Expr)>)>> {
        let log = self.log.as_ref()?;
        let mut terms: Vec<Expr> = vec![];
        for t in walk(e) {
            let is_read = is_total(t)
                || matches!(t, Expr::App(n, false, a) if a.len() == 1 && (n == "ledger.balance" || matches!(state_op(self.defs, n), Some((_, "get", _)))));
            if is_read && !terms.contains(t) {
                terms.push(t.clone());
            }
        }
        let mut out = vec![];
        for t in terms {
            let mut cases: Vec<(Vec<Expr>, Expr)> = vec![(vec![], t.clone())];
            if is_total(&t) {
                for w in log {
                    if let Write::Bal(_, op, n) = w {
                        cases[0].1 = bin(op, cases[0].1.clone(), n.clone());
                    }
                }
                out.push((t, cases));
                continue;
            }
            let Expr::App(name, _, args) = &t else { unreachable!() };
            let key = &args[0];
            let map = name.strip_suffix(".get");
            let hit = |w: &Write| -> Option<(Expr, Box<dyn Fn(&Expr) -> Expr>)> {
                match (w, map) {
                    (Write::Bal(a, op, n), None) => {
                        let (op, n) = (*op, n.clone());
                        Some((a.clone(), Box::new(move |v: &Expr| bin(op, v.clone(), n.clone()))))
                    }
                    (Write::Map(x, k, v), Some(m)) if x == m => {
                        let v = v.clone();
                        Some((k.clone(), Box::new(move |_: &Expr| v.clone())))
                    }
                    _ => None,
                }
            };
            let touched = log.iter().any(|w| hit(w).is_some());
            if touched && reads_state(key) {
                return None; // the key itself may have changed
            }
            for w in log {
                let Some((at, apply)) = hit(w) else { continue };
                let mut next = vec![];
                for (conds, v) in cases {
                    if Self::same(key, &at) {
                        next.push((conds, apply(&v)));
                    } else if self.differ(key, &at) {
                        next.push((conds, v));
                    } else {
                        let eq = bin("=", key.clone(), at.clone());
                        let ne = bin("!=", key.clone(), at.clone());
                        if !conds.contains(&ne) {
                            let mut c = conds.clone();
                            if !c.contains(&eq) {
                                c.push(eq.clone());
                            }
                            next.push((c, apply(&v)));
                        }
                        if !conds.contains(&eq) {
                            let mut c = conds;
                            if !c.contains(&ne) {
                                c.push(ne);
                            }
                            next.push((c, v));
                        }
                    }
                }
                cases = next;
                if cases.len() > MAX_CASES {
                    return None;
                }
            }
            out.push((t, cases));
        }
        Some(out)
    }

    /// A value in pre-state terms, only when it has exactly one (no case split).
    fn pre_value(&self, e: &Expr) -> Option<Expr> {
        let e2 = self.with_lets(e)?;
        if !reads_state(&e2) {
            return Some(e2);
        }
        let reads = self.read_cases(&e2)?;
        let mut m = vec![];
        for (t, cases) in reads {
            match cases.as_slice() {
                [(conds, v)] if conds.is_empty() => m.push((t, v.clone())),
                _ => return None,
            }
        }
        Some(replace_terms(&e2, &m))
    }

    /// A condition in pre-state terms: one disjunct per aliasing case, at most MAX_CASES.
    fn pre_cond(&self, e: &Expr) -> Option<Expr> {
        let e2 = self.with_lets(e)?;
        if !reads_state(&e2) {
            return Some(e2);
        }
        let reads = self.read_cases(&e2)?;
        let mut combos: Vec<(Vec<Expr>, Vec<(Expr, Expr)>)> = vec![(vec![], vec![])];
        for (t, cases) in &reads {
            let mut next = vec![];
            for (conds, m) in &combos {
                for (cs, v) in cases {
                    let mut c = conds.clone();
                    c.extend(cs.iter().filter(|x| !conds.contains(x)).cloned());
                    let mut m = m.clone();
                    m.push((t.clone(), v.clone()));
                    next.push((c, m));
                }
            }
            combos = next;
            if combos.len() > MAX_CASES {
                return None;
            }
        }
        let mut alts = combos.into_iter().map(|(conds, m)| {
            let body = replace_terms(&e2, &m);
            conds.into_iter().rev().fold(body, |acc, c| bin("and", c, acc))
        });
        let first = alts.next()?;
        Some(alts.fold(first, |acc, a| bin("or", acc, a)))
    }

    /// Record `a != b` refusal points: past them, a and b differ.
    fn learn(&mut self, cond: &Expr) {
        match cond {
            Expr::Bin("and", l, r) => {
                self.learn(l);
                self.learn(r);
            }
            Expr::Bin("!=", l, r) => {
                if let (Some(l), Some(r)) = (self.pre_value(l), self.pre_value(r)) {
                    self.distinct.push((l, r));
                }
            }
            _ => {}
        }
    }

    fn add(&mut self, cond: &Expr, ctor: &str, payload: &[Expr], origin: &str, guards: &[Option<Expr>]) {
        let lifted = self.pre_cond(cond);
        // the call refuses unless cond holds here, so later reads may use it (even when it can't be lifted)
        let learn_from = cond.clone();
        if let Some(mut c) = lifted {
            if guards.iter().all(|g| g.is_some()) {
                let mut ok = true;
                for g in guards.iter().rev() {
                    // inside `if G`: the call refuses when G and not cond, so callers may assume `not G or cond`
                    let Some(ng) = negate(g.as_ref().unwrap()) else {
                        ok = false;
                        break;
                    };
                    c = bin("or", ng, c);
                }
                if ok {
                    let p: Vec<Option<Expr>> = payload.iter().map(|a| self.pre_value(a)).collect();
                    let payload = if p.iter().any(|x| x.is_none()) { None } else { Some(p.into_iter().map(Option::unwrap).collect()) };
                    if self.out.iter().all(|o| !(o.cond == c && o.ctor == ctor)) {
                        self.out.push(Lifted { cond: c, ctor: ctor.to_string(), payload, origin: origin.to_string() });
                    }
                }
            }
        }
        self.learn(&learn_from);
    }

    fn walk(&mut self, stmts: &[Stmt], guards: &[Option<Expr>]) {
        for s in stmts {
            match s {
                Stmt::Let(x, e) => {
                    let v = self.pre_value(e);
                    let usable = v.is_some();
                    self.lets.insert(x.clone(), (v.unwrap_or_else(|| e.clone()), usable));
                }
                Stmt::Need(c, ctor, args) => self.add(c, ctor, args, "", guards),
                Stmt::Inv(_) => continue,
                Stmt::For(_, _, _, _, body) => {
                    // a loop's refusals depend on earlier iterations: never lifted; any effect ends lifting
                    if flat(body).iter().any(|x| matches!(x, Stmt::Do(_))) {
                        self.log = None;
                    }
                }
                Stmt::If(c, t, o) => {
                    let g = self.pre_cond(c);
                    let ng = g.as_ref().and_then(negate);
                    let (log0, distinct0) = (self.log.clone(), self.distinct.clone());
                    let mut g1 = guards.to_vec();
                    g1.push(g);
                    self.walk(t, &g1);
                    let log_then = std::mem::replace(&mut self.log, log0.clone());
                    self.distinct = distinct0.clone();
                    let mut g2 = guards.to_vec();
                    g2.push(ng);
                    self.walk(o, &g2);
                    // the after-state stays exact only if neither branch wrote (no merge of conditional writes)
                    if log_then != log0 || self.log != log0 {
                        self.log = None;
                    }
                    self.distinct = distinct0;
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
                            self.log = None;
                        }
                    } else if WRITERS.contains(&name.as_str()) {
                        let op = if name == "ledger.debit" { "-" } else { "+" };
                        let w = match (self.pre_value(&args[0]), self.pre_value(&args[1])) {
                            (Some(a), Some(n)) => Some(Write::Bal(a, op, n)),
                            _ => None,
                        };
                        self.push_write(w);
                    } else if let Some((x, "set", _)) = state_op(self.defs, name) {
                        let w = match (self.pre_value(&args[0]), self.pre_value(&args[1])) {
                            (Some(k), Some(v)) => Some(Write::Map(x.to_string(), k, v)),
                            _ => None,
                        };
                        self.push_write(w);
                    }
                }
                Stmt::Do(_) => {}
            }
        }
    }

    fn push_write(&mut self, w: Option<Write>) {
        match (w, self.log.as_mut()) {
            (Some(w), Some(log)) => log.push(w),
            _ => self.log = None,
        }
    }
}

fn lifted_seen(defs: &Module, d: &Def, seen: &BTreeSet<String>) -> Vec<Lifted> {
    let mut l = Lifter { defs, seen: with_seen(seen, &d.name), out: vec![], lets: IndexMap::new(), log: Some(vec![]), distinct: vec![] };
    // spec needs are refusal points before the body
    for n in &d.needs {
        if let Need::Need(c, _, _) = n {
            l.learn(c);
        }
    }
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
