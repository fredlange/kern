//! Well-formedness and type checking (port of kern/wf.py), run before anything is proven or executed.
//!
//! It rejects: unknown names, wrong arity, a variable applied to arguments; a `let` that shadows a parameter, an
//! earlier `let`, a handler or a definition; specs that read body `let`s, call write/audit handlers or definitions,
//! or use primes outside `ensure`; `_` outside the top level of an `emit` argument; recursion without a termination
//! measure (`decreases E`); type errors: Int, Account, Text, Owner, Bool, Event, Unit (e.g. `from + to`).
//!
//! Problems come out in exactly Python's order and wording: they are joined into the error text.

use indexmap::{IndexMap, IndexSet};

use crate::infer::{called_names, callees, calls_procedure, is_function, uses, walk};
use crate::run::{assumed_op, channel_op, is_handler, state_op};
use crate::syntax::{code, Def, Emit, Expr, Module, Need, ParseError, Stmt};

pub const PARAM_TYPES: [&str; 5] = ["Int", "Account", "Text", "List", "Decimal"];
const ANY: &str = "Any";

/// handler -> (argument types, result type)
pub fn sig(name: &str) -> Option<(&'static [&'static str], &'static str)> {
    Some(match name {
        "ledger.balance" => (&["Account"], "Int"),
        "ledger.owner" => (&["Account"], "Owner"),
        "ledger.total" => (&[], "Int"),
        "ledger.debit" => (&["Account", "Int"], "Unit"),
        "ledger.credit" => (&["Account", "Int"], "Unit"),
        "clock.now" => (&[], "Int"),
        "clock.tick" => (&[], "Unit"),
        "audit.emit" => (&["Event"], "Unit"),
        _ => return None,
    })
}

const SPEC_READS: [&str; 4] = ["ledger.balance", "ledger.owner", "ledger.total", "clock.now"]; // time is fixed per call
const NO_AFTER_STATE: [&str; 1] = ["clock.now"]; // ... so it has no `'`

/// The problems found, joined as Python's WellFormedError text.
#[derive(Clone, Debug, PartialEq)]
pub struct WellFormedError {
    pub problems: Vec<String>,
}

impl WellFormedError {
    pub fn message(&self) -> String {
        format!("not well-formed:\n  {}", self.problems.join("\n  "))
    }
}

impl From<WellFormedError> for ParseError {
    fn from(e: WellFormedError) -> ParseError {
        ParseError(e.message())
    }
}

type Scope = IndexMap<String, String>;

struct Checker<'a> {
    defs: &'a Module,
    d: &'a Def,
    out: Vec<String>,
    used: IndexSet<String>,
}

fn is_wild(e: &Expr) -> bool {
    matches!(e, Expr::Name(n, false) if n == "_")
}

impl<'a> Checker<'a> {
    fn new(defs: &'a Module, d: &'a Def) -> Self {
        Checker { defs, d, out: vec![], used: IndexSet::new() }
    }

    fn err(&mut self, place: &str, msg: &str) {
        self.out.push(format!("{}: {}: {}", self.d.name, place, msg));
    }

    // ---- expressions: return the type (or ANY after an error, to avoid cascades)
    fn ty(&mut self, e: &Expr, scope: &Scope, place: &str, spec: bool, primed_ok: bool, wild_ok: bool) -> String {
        match e {
            Expr::Int(_) => "Int".into(),
            Expr::Text(_) => "Text".into(),
            Expr::Bin(op, l, r) => {
                let lt = self.ty(l, scope, place, spec, primed_ok, false);
                let rt = self.ty(r, scope, place, spec, primed_ok, false);
                match *op {
                    // `+` on two Texts joins them
                    "+" if lt == "Text" || rt == "Text" => self.need_types(place, op, &[&lt, &rt], &["Text", "Text"], "Text"),
                    // a Decimal with a Decimal or an Int (promoted exactly); `/` by a positive Int literal (floor)
                    "+" | "-" | "*" if lt == "Decimal" || rt == "Decimal" => {
                        if ![&lt, &rt].iter().all(|t| ["Decimal", "Int", ANY].contains(&t.as_str())) {
                            self.err(place, &format!("`{}`: {} {} {}", code(e), lt, op, rt));
                        }
                        "Decimal".into()
                    }
                    "/" if lt == "Decimal" => {
                        if !matches!(**r, Expr::Int(n) if n > 0) {
                            self.err(place, &format!("`{}`: `/` needs a positive number on the right", code(e)));
                        }
                        "Decimal".into()
                    }
                    ">=" | "<=" | ">" | "<" | "=" | "!=" if (lt == "Decimal" && (rt == "Int" || rt == "Decimal")) || (rt == "Decimal" && lt == "Int") => {
                        "Bool".into()
                    }
                    "+" | "-" => self.need_types(place, op, &[&lt, &rt], &["Int", "Int"], "Int"),
                    "*" | "/" => {
                        // linear arithmetic only, so the prover stays decidable
                        if *op == "/" && !matches!(**r, Expr::Int(n) if n > 0) {
                            self.err(place, &format!("`{}`: `/` needs a positive number on the right", code(e)));
                        }
                        self.need_types(place, op, &[&lt, &rt], &["Int", "Int"], "Int")
                    }
                    ">=" | "<=" | ">" | "<" => self.need_types(place, op, &[&lt, &rt], &["Int", "Int"], "Bool"),
                    "and" | "or" => self.need_types(place, op, &[&lt, &rt], &["Bool", "Bool"], "Bool"),
                    "when" => {
                        self.err(place, "`when` is only allowed at the end of an `ensure` or `emit`");
                        "Bool".into()
                    }
                    _ => {
                        if lt != ANY && rt != ANY && (lt != rt || lt == "Unit") {
                            self.err(place, &format!("`{}` compares {} with {}", code(e), lt, rt));
                        }
                        "Bool".into()
                    }
                }
            }
            Expr::Name(name, primed) => {
                if is_wild(e) {
                    if !wild_ok {
                        self.err(place, "`_` is only allowed as a top-level `emit` argument");
                    }
                    return ANY.into();
                }
                // the Account an unset entry of an Account-valued state holds
                if name == "none" && !scope.contains_key(name) {
                    return "Account".into();
                }
                if name == "[]" {
                    return "List".into();
                }
                if name.starts_with(|c: char| c.is_ascii_digit()) && crate::syntax::decimal_value(name).is_ok() {
                    return "Decimal".into();
                }
                self.apply(name, *primed, &[], scope, place, spec, primed_ok)
            }
            Expr::App(name, primed, args) => self.apply(name, *primed, args, scope, place, spec, primed_ok),
            Expr::All(..) => {
                self.err(place, "`for j from A to B` is only allowed at the end of an `ensure` or a loop `invariant`");
                "Bool".into()
            }
            Expr::Frame(x, range) => {
                match self.defs.states.get(x) {
                    // `keep NAME` where NAME isn't a state: say what it is, an unknown name (body lets included)
                    None if range.is_none() => self.err(place, &format!("unknown name `{}`", x)),
                    None => self.err(place, &format!("`keep {}`: {} is not a declared state", code(e), x)),
                    Some(kt) if range.is_some() && kt != "Int" => {
                        self.err(place, &format!("`keep {}`: a range frame needs an Int-keyed state, {} is {}-keyed", code(e), x, kt))
                    }
                    _ => {}
                }
                if !primed_ok {
                    self.err(place, "`keep X [except A to B]` is a spec clause");
                }
                if let Some((a, b)) = range {
                    for bnd in [a, b] {
                        let t = self.ty(bnd, scope, place, spec, false, false);
                        self.expect(place, &t, "Int");
                    }
                }
                "Bool".into()
            }
            Expr::Sum(j, a, b, body, cond) => {
                for bnd in [a, b] {
                    let t = self.ty(bnd, scope, place, spec, primed_ok, false);
                    self.expect(place, &t, "Int");
                }
                if scope.contains_key(j) || self.defs.defs.contains_key(j) {
                    self.err(place, &format!("`{}` shadows an existing name", j));
                }
                let inner: Vec<&Expr> = std::iter::once(&**body).chain(cond.as_deref()).collect();
                for x in &inner {
                    for n in crate::infer::walk(x) {
                        match n {
                            Expr::Sum(..) => self.err(place, "a sum inside a sum is not supported yet"),
                            Expr::App(h, _, _) | Expr::Name(h, _)
                                if self.defs.defs.contains_key(h) || assumed_op(self.defs, h).is_some() =>
                            {
                                self.err(place, &format!("a sum's body may not call `{}`", h))
                            }
                            _ => {}
                        }
                    }
                }
                let mut scope = scope.clone();
                scope.insert(j.clone(), "Int".into());
                let t = self.ty(body, &scope, place, spec, primed_ok, false);
                self.expect(place, &t, "Int");
                if let Some(c) = cond {
                    let t = self.ty(c, &scope, place, spec, primed_ok, false);
                    self.expect(place, &t, "Bool");
                }
                "Int".into()
            }
        }
    }

    fn need_types(&mut self, place: &str, op: &str, got: &[&String], want: &[&str], result: &str) -> String {
        if !got.iter().any(|g| *g == ANY) && got.iter().map(|g| g.as_str()).ne(want.iter().copied()) {
            let g: Vec<&str> = got.iter().map(|g| g.as_str()).collect();
            self.err(place, &format!("`{}` needs {}, got {}", op, want.join(" and "), g.join(" and ")));
        }
        result.into()
    }

    #[allow(clippy::too_many_arguments)]
    fn apply(&mut self, name: &str, primed: bool, args: &[Expr], scope: &Scope, place: &str, spec: bool,
             primed_ok: bool) -> String {
        if primed && !primed_ok {
            self.err(place, &format!("`{}'` (after-state) is only allowed in `ensure`", name));
        }
        let arg_types: Vec<String> = args.iter().map(|a| self.ty(a, scope, place, spec, primed_ok, false)).collect();
        if let Some(t) = scope.get(name) {
            if !args.is_empty() {
                self.err(place, &format!("`{}` is a value, not a function", name));
            }
            if primed {
                self.err(place, &format!("`{}'`: only handler reads have an after-state", name));
            }
            return t.clone();
        }
        let defs = self.defs;
        if is_handler(defs, name) {
            if primed && NO_AFTER_STATE.contains(&name) {
                self.err(place, &format!("`{}'`: the time is the same before and after a call", name));
            }
            let st = state_op(defs, name);
            let reads = SPEC_READS.contains(&name) || matches!(st, Some((_, "get", _)));
            if spec && !reads {
                self.err(place, &format!("specs may only read state; `{}` has an effect", name));
            }
            let (want, result): (Vec<String>, &str) = if let Some(h) = assumed_op(defs, name) {
                // an observation: not a pure read
                if spec {
                    self.err(place, &format!("specs cannot observe; `{}` is an assumed handler", name));
                }
                (h.params.iter().map(|(_, t)| t.clone()).collect(), "Int")
            } else if channel_op(defs, name).is_some() {
                (vec!["Event".into()], "Unit")
            } else if let Some((_, op, kt)) = st {
                // a state's values are Int, or Text when declared `-> Text`
                let x = st.map(|s| s.0);
                let vt = if defs.text_states.iter().any(|t| Some(t.as_str()) == x) {
                    "Text"
                } else if defs.account_states.iter().any(|t| Some(t.as_str()) == x) {
                    "Account"
                } else if defs.list_states.iter().any(|t| Some(t.as_str()) == x) {
                    "List"
                } else if defs.decimal_states.iter().any(|t| Some(t.as_str()) == x) {
                    "Decimal"
                } else {
                    "Int"
                };
                if op == "get" {
                    (vec![kt.to_string()], vt)
                } else {
                    (vec![kt.to_string(), vt.into()], "Unit")
                }
            } else {
                let (w, r) = sig(name).unwrap();
                (w.iter().map(|s| s.to_string()).collect(), r)
            };
            return self.check_args(place, name, &want, &arg_types, result);
        }
        if let Some(callee) = defs.defs.get(name) {
            let fn_ = callee.ret.is_some(); // `-> Int`: it gives a value
            if spec && !is_function(defs, callee) {
                self.err(
                    place,
                    &format!("specs may call only functions (`-> Int`, reading state, never refusing), not `{}`", name),
                );
            }
            if primed {
                self.err(place, &format!("`{}'`: only handler reads have an after-state", name));
            }
            let want: Vec<String> = callee.params.iter().map(|(_, t)| t.clone()).collect();
            let ret = if callee.ret.as_deref() == Some("Decimal") { "Decimal" } else if fn_ { "Int" } else { "Unit" };
            return self.check_args(place, name, &want, &arg_types, ret);
        }
        if name == "length" {
            // `length t`: the number of characters of a Text, or the number of elements of a List
            let want = if arg_types.first().map(String::as_str) == Some("List") { "List" } else { "Text" };
            return self.check_args(place, name, &[want.to_string()], &arg_types, "Int");
        }
        if name == "at" {
            // `at xs i`: the i-th element, from 0 (a call-site goal says i is in range)
            return self.check_args(place, name, &["List".to_string(), "Int".to_string()], &arg_types, "Int");
        }
        if name == "append" {
            // `append xs v`: xs with v at the end
            return self.check_args(place, name, &["List".to_string(), "Int".to_string()], &arg_types, "List");
        }
        if name == "distinct" {
            // `distinct a b c`: pairwise different
            let kinds: IndexSet<&String> = arg_types.iter().filter(|t| *t != ANY).collect();
            if args.len() < 2 || kinds.len() > 1 {
                self.err(place, "`distinct` takes two or more values of one type");
            }
            return "Bool".into();
        }
        if name.chars().next().map_or(false, |c| c.is_uppercase()) {
            for (a, t) in args.iter().zip(&arg_types) {
                if t == "Unit" {
                    self.err(place, &format!("`{}` has no value (Unit) and cannot be a constructor argument", code(a)));
                }
            }
            return "Event".into();
        }
        self.err(place, &format!("unknown name `{}`", name));
        ANY.into()
    }

    fn check_args(&mut self, place: &str, name: &str, want: &[String], got: &[String], result: &str) -> String {
        if want.len() != got.len() {
            self.err(place, &format!("`{}` takes {} argument(s), got {}", name, want.len(), got.len()));
            return result.into();
        }
        for (i, (w, g)) in want.iter().zip(got).enumerate() {
            // an Int where a Decimal is wanted is promoted exactly
            if g != ANY && w != g && !(w == "Decimal" && g == "Int") {
                self.err(place, &format!("argument {} of `{}` must be {}, got {}", i + 1, name, w, g));
            }
        }
        result.into()
    }

    fn expect(&mut self, place: &str, t: &str, want: &str) {
        if t != ANY && t != want {
            self.err(place, &format!("must be {}, got {}", want, t));
        }
    }

    // ---- a whole definition
    fn run(mut self) -> Vec<String> {
        let d = self.d;
        let mut params = Scope::new();
        for (p, t) in &d.params {
            if !PARAM_TYPES.contains(&t.as_str()) {
                self.err("header", &format!("parameter type `{}` is not supported (Int, Account, Text, List)", t));
            }
            if params.contains_key(p) {
                self.err("header", &format!("parameter `{}` appears twice", p));
            }
            params.insert(p.clone(), if PARAM_TYPES.contains(&t.as_str()) { t.clone() } else { ANY.into() });
            if p == "length" {
                self.err("header", "`length` is built in (the length of a Text); name the parameter otherwise");
            }
        }
        if d.name == "length" {
            self.err("header", "`length` is built in (the length of a Text); name the definition otherwise");
        }
        if let Some(r) = &d.ret {
            if r != "Int" && r != "Decimal" && !self.defs.records.contains_key(r) {
                self.err("header", "a definition returns Int, Decimal or a record (`-> Int`, `-> Point`)");
            }
            // one that writes, emits or refuses is a procedure returning a value: inferred, not written
            // (DECISIONS.md, 2026-11-06)
            if !assigns(&d.body) {
                self.err("body", "every path must set `result`");
            }
            // specs (ensure) may name the result; a record result by its fields, `result.x`
            if self.defs.records.contains_key(r) {
                for (f, t) in crate::syntax::record_leaves(&self.defs.records, r) {
                    params.insert(format!("result.{f}"), t);
                }
            } else {
                params.insert("result".into(), r.clone());
            }
        }
        if let Some(m) = &d.decreases {
            let place = format!("decreases {}", code(m));
            let t = self.ty(m, &params, &place, true, false, false);
            self.expect(&place, &t, "Int");
            // over the parameters only: a measure that read state could change under the call it bounds
            for n in crate::infer::names(m) {
                let reads_state = is_handler(self.defs, &n)
                    || self.defs.defs.get(&n).is_some_and(|f| !uses(self.defs, f).is_empty() || !is_function(self.defs, f));
                if reads_state {
                    self.err(&place, &format!("a measure may read only parameters, constants and pure functions, not `{}`", n));
                }
            }
        }
        for n in &d.needs {
            let Need::Need(c, _, args) = n else { continue };
            let place = format!("need {}", code(c));
            let t = self.ty(c, &params, &place, true, false, false);
            self.expect(&place, &t, "Bool");
            for a in args {
                let t = self.ty(a, &params, &place, true, false, false);
                if t == "Text" || t == "List" || t == "Decimal" {
                    self.err(&place, &format!("failure payload `{}` is {}; payloads are Int or Account", code(a), t));
                }
            }
        }
        for c in &d.keeps {
            let place = format!("keep {}", code(c));
            let t = self.ty(c, &params, &place, true, false, false);
            if t == "Unit" || t == "Bool" {
                self.err(&place, &format!("must be a value, got {}", t));
            }
        }
        for c in &d.ensures {
            self.clause(c, &params, &format!("ensure {}", code(c)));
        }
        for (i, ch) in &d.emit_channel {
            if !self.defs.channels.contains(ch) {
                let ctor = match &d.emits[*i] {
                    Emit::Ev(c, _, _) => c.clone(),
                    Emit::As(..) => "emit_as".into(),
                };
                self.err(&format!("emit {} {}", ch, ctor), &format!("no channel named `{}` (declare `channel {}`)", ch, ch));
            }
        }
        for ev in &d.emits {
            let Emit::Ev(ctor, args, cond) = ev else { continue };
            let place = format!("emit {}", ctor);
            for a in args {
                self.ty(a, &params, &place, true, false, true);
            }
            if let Some(c) = cond {
                let t = self.ty(c, &params, &place, true, false, false);
                self.expect(&place, &t, "Bool");
            }
        }
        let mut scope = params.clone();
        self.block(&d.body, &mut scope, false);
        self.out
    }

    /// `E [when C] [for j from A to B]`: E may use after-state reads, C is about the before-state.
    fn clause(&mut self, c: &Expr, scope: &Scope, place: &str) {
        let mut scope = scope.clone();
        let mut c = c;
        if let Expr::All(j, a, b, body) = c {
            for bnd in [a, b] {
                let t = self.ty(bnd, &scope, place, true, false, false);
                self.expect(place, &t, "Int");
            }
            if scope.contains_key(j) || self.defs.defs.contains_key(j) {
                self.err(place, &format!("`{}` shadows an existing name", j));
            }
            scope.insert(j.clone(), "Int".into());
            c = body;
        }
        if let Expr::Bin("when", e, cond) = c {
            let t = self.ty(e, &scope, place, true, true, false);
            self.expect(place, &t, "Bool");
            let t = self.ty(cond, &scope, place, true, false, false);
            self.expect(place, &t, "Bool");
        } else {
            let t = self.ty(c, &scope, place, true, true, false);
            self.expect(place, &t, "Bool");
        }
    }

    fn shadows(&self, x: &str, scope: &Scope) -> bool {
        scope.contains_key(x) || self.used.contains(x) || is_handler(self.defs, x) || self.defs.defs.contains_key(x)
    }

    /// A definition that changes state (or refuses) and returns a value is called only as a whole statement, `x = f a`
    /// or `f a`, so effects happen in statement order: never inside an expression, a condition or a bound.
    fn calls_on_their_own(&mut self, s: &Stmt) {
        let (top, exprs): (Option<&Expr>, Vec<&Expr>) = match s {
            Stmt::Let(_, e) | Stmt::Do(e) => (Some(e), vec![e]),
            Stmt::Need(c, _, a) => (None, std::iter::once(c).chain(a.iter()).collect()),
            Stmt::If(c, _, _) => (None, vec![c]),
            Stmt::For(_, lo, hi, _, _) => (None, vec![lo, hi]),
            Stmt::Inv(_) => (None, vec![]),
        };
        for e in exprs {
            for n in walk(e) {
                if top.is_some_and(|t| std::ptr::eq(t, n)) {
                    continue;
                }
                if let Some(f) = calls_procedure(self.defs, n) {
                    if self.defs.defs[&f].ret.is_some() {
                        let msg = format!("`{}` changes state or refuses: call it on a line of its own, `x = {}`", f, code(n));
                        self.err(&code(e), &msg);
                    }
                }
            }
        }
    }

    fn block(&mut self, stmts: &[Stmt], scope: &mut Scope, in_loop: bool) {
        for s in stmts {
            self.calls_on_their_own(s);
            match s {
                Stmt::Inv(e) => {
                    self.err(&format!("invariant {}", code(e)), "an `invariant` line belongs at the start of a `for` loop")
                }
                Stmt::For(k, lo, hi, invs, body) => {
                    let place = format!("for {} from {} to {}", k, code(lo), code(hi));
                    for b in [lo, hi] {
                        let t = self.ty(b, scope, &place, false, false, false);
                        self.expect(&place, &t, "Int");
                    }
                    if self.shadows(k, scope) {
                        self.err(&place, &format!("`{}` shadows an existing name", k));
                    }
                    self.used.insert(k.clone());
                    let mut inner = scope.clone();
                    inner.insert(k.clone(), "Int".into());
                    for inv in invs {
                        self.clause(inv, &inner, &format!("invariant {}", code(inv)));
                    }
                    self.block(body, &mut inner, true);
                }
                Stmt::If(c, t, o) => {
                    let place = format!("if {}", code(c));
                    let ty = self.ty(c, scope, &place, false, false, false);
                    self.expect(&place, &ty, "Bool");
                    // a branch's names stay in the branch, but are never reused (self.used)
                    self.block(t, &mut scope.clone(), in_loop);
                    self.block(o, &mut scope.clone(), in_loop);
                }
                Stmt::Let(x, e) => {
                    let place = format!("{} = {}", x, code(e));
                    let t = self.ty(e, scope, &place, false, false, false);
                    if x == "result" && self.d.ret.is_some() {
                        // each path sets it once (checked by assigns)
                        scope.shift_remove("result");
                    }
                    if self.shadows(x, scope) {
                        self.err(&place, &format!("`{}` shadows an existing name", x));
                    }
                    if t == "Unit" {
                        self.err(&place, "has no value (Unit)");
                    }
                    // a record value (built at load time as its constructor, or a call returning one): its fields
                    let rec = match e {
                        Expr::App(n, false, _) if self.defs.records.contains_key(n) => Some(n.clone()),
                        Expr::App(n, false, _) | Expr::Name(n, false) => {
                            self.defs.defs.get(n).and_then(|f| f.ret.clone()).filter(|r| self.defs.records.contains_key(r))
                        }
                        _ => None,
                    };
                    // `result = …` has the declared type: a record of that type for `-> Point`, an Int for `-> Int`
                    if x == "result" {
                        if let Some(want) = &self.d.ret {
                            let got = rec.clone().unwrap_or_else(|| t.clone());
                            if got != *want && got != ANY && !(want == "Decimal" && got == "Int") {
                                self.err(&place, &format!("the result is declared {}, this is {}", want, got));
                            }
                        }
                    }
                    if let Some(r) = rec {
                        for (f, ft) in crate::syntax::record_leaves(&self.defs.records, &r) {
                            scope.insert(format!("{x}.{f}"), ft);
                        }
                        if x != "result" {
                            self.used.insert(x.clone());
                        }
                        continue;
                    }
                    scope.insert(x.clone(), t);
                    if x != "result" {
                        self.used.insert(x.clone());
                    }
                }
                Stmt::Need(c, _, args) => {
                    let place = format!("need {}", code(c));
                    let t = self.ty(c, scope, &place, false, false, false);
                    self.expect(&place, &t, "Bool");
                    for a in args {
                        let t = self.ty(a, scope, &place, false, false, false);
                        if t == "Unit" {
                            self.err(&place, &format!("failure payload `{}` has no value (Unit)", code(a)));
                        }
                        if t == "Text" || t == "List" || t == "Decimal" {
                            self.err(&place, &format!("failure payload `{}` is {}; payloads are Int or Account", code(a), t));
                        }
                    }
                }
                Stmt::Do(e) => {
                    let place = code(e);
                    let t = self.ty(e, scope, &place, false, false, false);
                    // a procedure's value may be dropped (`borrow book`): the call is for its effect
                    if t != "Unit" && t != ANY && calls_procedure(self.defs, e).is_none() {
                        self.err(&place, &format!("value of type {} is computed and discarded", t));
                    }
                }
            }
        }
    }
}

/// Every path through stmts sets `result`.
fn assigns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Let(x, _) => x == "result",
        Stmt::If(_, t, o) => assigns(t) && assigns(o),
        _ => false,
    })
}

/// Problems with one definition (used for mutants as well as source).
pub fn problems(defs: &Module, d: &Def) -> Vec<String> {
    Checker::new(defs, d).run()
}

pub fn cycles(defs: &Module) -> Vec<String> {
    let mut out = vec![];
    let mut state: IndexMap<String, bool> = IndexMap::new(); // true = open, false = done

    fn visit(defs: &Module, n: &str, path: Vec<String>, state: &mut IndexMap<String, bool>, out: &mut Vec<String>) {
        state.insert(n.to_string(), true);
        let d = &defs.defs[n];
        let mut next: Vec<String> = if called_names(d).iter().any(|x| x == n) { vec![n.to_string()] } else { vec![] };
        next.extend(callees(defs, d)); // callees() leaves out self-calls
        for c in next {
            match state.get(&c) {
                Some(true) => {
                    let i = path.iter().position(|x| *x == c).unwrap();
                    let mut lp: Vec<String> = path[i..].to_vec();
                    lp.push(c.clone());
                    // accepted when every definition on the cycle states a measure (the prover checks it)
                    if let Some(m) = lp.iter().find(|x| defs.defs[x.as_str()].decreases.is_none()) {
                        out.push(format!(
                            "{}: recursive ({}); {} needs `decreases E`, an Int that stays >= 0 and is smaller at every recursive call",
                            lp[0],
                            lp.join(" -> "),
                            m
                        ));
                    }
                }
                None => {
                    let mut p = path.clone();
                    p.push(c.clone());
                    visit(defs, &c, p, state, out);
                }
                Some(false) => {}
            }
        }
        state.insert(n.to_string(), false);
    }

    for n in defs.defs.keys() {
        if !state.contains_key(n) {
            visit(defs, n, vec![n.clone()], &mut state, &mut out);
        }
    }
    out
}

/// Sub-expressions without descending into quantifiers (Python's `_walk_expr`).
fn walk_expr(e: &Expr) -> Vec<&Expr> {
    let mut out = vec![e];
    match e {
        Expr::Bin(_, l, r) => {
            out.extend(walk_expr(l));
            out.extend(walk_expr(r));
        }
        Expr::App(_, _, args) => {
            for a in args {
                out.extend(walk_expr(a));
            }
        }
        _ => {}
    }
    out
}

pub fn invariant_problems(defs: &Module) -> Vec<String> {
    let mut out = vec![];
    for (var, t, cond) in &defs.invariants {
        let d = Def::new(&format!("invariant {}", var), vec![], None);
        let mut c = Checker::new(defs, &d);
        let mut scope = Scope::new();
        scope.insert(var.clone(), t.clone());
        let place = code(cond);
        let ty = c.ty(cond, &scope, &place, true, false, false);
        c.expect(&place, &ty, "Bool");
        out.extend(c.out);
        // Checked key by key (prover: at parameter keys; host: at written keys), which is only sound if the
        // invariant reads state at its own bound key and nowhere else.
        for e in walk_expr(cond) {
            let (name, args): (&str, &[Expr]) = match e {
                Expr::Name(n, _) => (n, &[]),
                Expr::App(n, _, a) => (n, a),
                _ => continue,
            };
            if name == var || !is_handler(defs, name) {
                continue;
            }
            if !(args.len() == 1 && args[0] == Expr::Name(var.clone(), false)) {
                out.push(format!("invariant {}: {}: `{}` must read state at `{}` only", var, code(cond), code(e), var));
            }
        }
    }
    out
}

/// An assumed spec may read only its arguments and `result`: anything else (ledger or state reads) would let an
/// assumption carry an unproven claim about owned state (kern-90).
pub fn handler_problems(defs: &Module) -> Vec<String> {
    let mut out = vec![];
    for (name, h) in &defs.handlers {
        if h.ret.as_deref() != Some("Int") {
            out.push(format!("handler {}: must return Int (`-> Int`)", name));
        }
        if !h.keeps.is_empty() || !h.emits.is_empty() || !h.examples.is_empty() || h.complete {
            out.push(format!("handler {}: only `need` and `ensure` lines (an assumed spec)", name));
        }
        let mut allowed: IndexSet<&str> = h.params.iter().map(|(p, _)| p.as_str()).collect();
        allowed.insert("result");
        let need_conds: Vec<&Expr> =
            h.needs.iter().filter_map(|n| if let Need::Need(c, _, _) = n { Some(c) } else { None }).collect();
        for e in need_conds.iter().copied().chain(h.ensures.iter()) {
            for x in walk_expr(e) {
                let n = match x {
                    Expr::Name(n, _) | Expr::App(n, _, _) => n.as_str(),
                    _ => continue,
                };
                if !n.is_empty()
                    && !allowed.contains(n)
                    && !n.chars().next().map_or(false, |c| c.is_uppercase())
                    && n != "distinct"
                {
                    out.push(format!(
                        "handler {}: `{}`: an assumed spec may read only its arguments and `result`",
                        name,
                        code(x)
                    ));
                }
            }
        }
        let mut c = Checker::new(defs, h);
        for (_, t) in &h.params {
            if !PARAM_TYPES.contains(&t.as_str()) {
                out.push(format!("handler {}: parameter type `{}` is not supported", name, t));
            }
        }
        let mut scope: Scope = h.params.iter().cloned().collect();
        scope.insert("result".into(), "Int".into());
        for cond in &need_conds {
            let place = code(cond);
            let t = c.ty(cond, &scope, &place, true, false, false);
            c.expect(&place, &t, "Bool");
        }
        for e in &h.ensures {
            c.clause(e, &scope, &code(e));
        }
        out.extend(c.out);
    }
    out
}

/// Every problem in the module, in Python's order.
pub fn module_problems(defs: &Module) -> Vec<String> {
    let mut found: Vec<String> = defs.defs.values().flat_map(|d| problems(defs, d)).collect();
    found.extend(cycles(defs));
    found.extend(invariant_problems(defs));
    found.extend(handler_problems(defs));
    found
}

pub fn check_module(defs: &Module) -> Result<(), WellFormedError> {
    let found = module_problems(defs);
    if found.is_empty() {
        Ok(())
    } else {
        Err(WellFormedError { problems: found })
    }
}
