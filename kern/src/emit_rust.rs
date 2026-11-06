//! Kern -> Rust (port of kern/emit_rust.py). Turns a checked Kern module into a Rust crate that links with the
//! runtime crate kern-rt (runtime/).
//!
//! ```text
//! kern build FILE.k [--out DIR]      (a file, a package or its directory; crates and cargo's target go in build/)
//! ```
//!
//! Each definition becomes one Rust function over a `Tx` (the runtime's call in progress). `need`s are behaviour and
//! are compiled; `keep`/`ensure`/`emit` are proven, so they compile to nothing. The semantics are the interpreter's
//! (docs/research/RUNTIME_PLAN.md has the contract); validation/difftest.py checks the generated program against it.
//!
//! The generated text is byte-identical to Python's (checked against the Python emitter until the 2026-11-05 cutover), so every existing build hash
//! and binary stays the same. An installed `kern` carries kern-rt's sources inside it, so `kern build` works without a
//! checkout (see `runtime_dir`).

use std::path::Path;

use indexmap::IndexMap;

use crate::infer::uses as infer_uses;
use crate::package::{abspath, load, root_of};
use crate::run::{assumed_op, channel_op, handler, state_op};
use crate::syntax::{code, Def, Expr, Module, Need, ParseError, Stmt};
use crate::wf::sig;

const RUST_KEYWORDS: &str = "as break const continue crate else enum extern false fn for if impl in let loop match mod
move mut pub ref return self static struct super trait true type unsafe use where while async await dyn
abstract become box do final macro override priv typeof unsized virtual yield try gen";

/// A construct the generator does not compile (yet). Never silently mistranslated.
#[derive(Clone, Debug, PartialEq)]
pub struct Unsupported(pub String);

#[derive(Debug)]
pub enum BuildError {
    Unsupported(String),
    Parse(ParseError),
    Runtime(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BuildError::Unsupported(m) | BuildError::Runtime(m) => f.write_str(m),
            BuildError::Parse(e) => f.write_str(&e.0),
        }
    }
}

impl From<Unsupported> for BuildError {
    fn from(u: Unsupported) -> BuildError {
        BuildError::Unsupported(u.0)
    }
}

impl From<ParseError> for BuildError {
    fn from(e: ParseError) -> BuildError {
        BuildError::Parse(e)
    }
}

type G<T> = Result<T, Unsupported>;

fn unsupported<T>(m: impl Into<String>) -> G<T> {
    Err(Unsupported(m.into()))
}

fn rust_type(t: &str) -> Option<&'static str> {
    Some(match t {
        "Int" => "i64",
        "Account" => "Acct",
        "Owner" => "Owner",
        "Bool" => "bool",
        "Text" => "String",
        _ => return None,
    })
}

pub fn ident(name: &str) -> String {
    // Injective, and disjoint from the generator's own names (tx, rt, w, exec, prov, args, __a0, __d0, __r, __h…,
    // __inv_…, a sum's __lo __hi __s __j): a usable Kern name is kept as it is unless it is a Rust keyword or one of those; anything else
    // (keyword, internal name, dotted record field `p.x`, handler `rand.below`) becomes `_` + a prefix-free code
    // (`_` -> `__`, `.` -> `_0`). No kept name starts with `_`; a coded one starts with `_` then a letter, a digit
    // or `__`/`_0`, never `_a`/`_d`/`_h`/`_i`/`_r` (`_` + a letter isn't a code), so the two never meet:
    // `p.x` -> `_p_0x`, `w` -> `_w`, while a Kern `p_x` or `w_` stays as it is.
    let reserved = RUST_KEYWORDS.split_whitespace().any(|k| k == name)
        || ["tx", "rt", "w", "exec", "prov", "args"].contains(&name)
        || name.starts_with('_')
        || name.contains('.');
    if !reserved {
        return name.to_string();
    }
    let mut out = String::from("_");
    for c in name.chars() {
        match c {
            '_' => out.push_str("__"),
            '.' => out.push_str("_0"),
            c => out.push(c),
        }
    }
    out
}

/// A constructor's Rust variant name. Kern constructors are Capitalised words; `Self` is the one that is a Rust keyword,
/// and no constructor starts with `_`, so `_Self` meets nothing. What the program prints stays `Self`.
pub fn variant(name: &str) -> String {
    if name == "Self" {
        "_Self".into()
    } else {
        name.to_string()
    }
}

/// Python's json.dumps of a string (ensure_ascii: non-ASCII as \uXXXX, surrogate pairs above the BMP).
pub fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// str.format with positional `{}` placeholders, in one pass (inserted text is never re-read).
fn fill(fmt: &str, vals: &[String]) -> String {
    let mut out = String::new();
    let mut rest = fmt;
    let mut i = 0;
    while let Some(p) = rest.find("{}") {
        out.push_str(&rest[..p]);
        out.push_str(&vals[i]);
        i += 1;
        rest = &rest[p + 2..];
    }
    out.push_str(rest);
    out
}

type Scope = IndexMap<String, String>;

pub struct Gen<'a> {
    defs: &'a Module,
    modname: String,
    /// ctor -> arg types, from `need ... else C args`
    refusals: IndexMap<String, Vec<String>>,
    /// ctor -> arg types, from constructor values
    events: IndexMap<String, Vec<String>>,
    channels: Vec<String>,
    /// (name, key type, slot, holds Text): Int maps and Text maps are numbered apart, per key type
    maps: Vec<(String, String, usize, bool)>,
    slot: IndexMap<String, (String, usize, bool)>,
    /// definition -> the module invariants (indices into `invariants`) its top-level calls check at the keys they
    /// wrote: those the prover did not prove for it
    check: IndexMap<String, Vec<usize>>,
}

impl<'a> Gen<'a> {
    pub fn new(defs: &'a Module, modname: &str) -> Self {
        let mut channels = vec!["audit".to_string()];
        channels.extend(defs.channels.iter().cloned());
        let (mut maps, mut slots) = (vec![], IndexMap::<(String, bool), usize>::new());
        for (name, key) in &defs.states {
            let text = defs.text_states.contains(name);
            let s = slots.entry((key.clone(), text)).or_insert(0);
            maps.push((name.clone(), key.clone(), *s, text));
            *s += 1;
        }
        let slot = maps.iter().map(|(n, k, s, t)| (n.clone(), (k.clone(), *s, *t))).collect();
        Gen {
            defs,
            modname: modname.into(),
            refusals: IndexMap::new(),
            events: IndexMap::new(),
            channels,
            maps,
            slot,
            check: IndexMap::new(),
        }
    }

    /// Check these module invariants after every top-level call of these definitions (`emit` passes
    /// `unproven_invariants`). Without it the program checks none: the bare interpreter's semantics (fuzzdiff).
    pub fn checking(mut self, check: IndexMap<String, Vec<usize>>) -> Self {
        self.check = check.into_iter().filter(|(_, is)| !is.is_empty()).collect();
        self
    }

    /// `__inv_NAME`: the module invariants NAME doesn't prove, at each key the call wrote; the first broken one is a
    /// fault, which undoes the call (as `kern host` does; sound because wf::invariant_problems keeps an invariant to
    /// its own key).
    fn invariant_check(&mut self, name: &str, which: &[usize]) -> G<Vec<String>> {
        let defs = self.defs;
        let mut out = vec![
            format!("/// {}: the module invariants it doesn't prove, at the keys the call wrote", name),
            format!("fn __inv_{}(tx: &mut Tx<Event>) -> R<()> {{", ident(name)),
        ];
        for (var, t, cond) in which.iter().map(|i| &defs.invariants[*i]) {
            let sc: Scope = IndexMap::from([(var.clone(), t.clone())]);
            let (c, _) = self.expr(cond, &sc)?;
            let text = json_str(&format!("invariant broken: {}:{} {} at ", var, t, code(cond)));
            let (keys, show) = if t == "Account" {
                ("tx.written_accts()", format!("tx.acct_name({}).to_string()", ident(var)))
            } else {
                ("tx.written_ints()", format!("{}.to_string()", ident(var)))
            };
            out.push(format!("    for {} in {} {{", ident(var), keys));
            out.push(format!("        if !({}) {{ return Err(Fault::Error({}.to_string() + &{}).into()); }}", c, text, show));
            out.push("    }".into());
        }
        out.push("    Ok(())".into());
        out.push("}".into());
        Ok(out)
    }

    /// one Rust variant per Kern constructor, with consistent argument types
    fn ctor(table: &mut IndexMap<String, Vec<String>>, name: &str, types: &[String], place: &str) -> G<()> {
        if types.iter().any(|t| rust_type(t).is_none()) {
            return unsupported(format!("{}: constructor {} with a {} argument", place, name, types.join("/")));
        }
        let prev = table.entry(name.to_string()).or_insert_with(|| types.to_vec());
        if prev != types {
            return unsupported(format!(
                "{}: constructor {} used with {} and with {}",
                place,
                name,
                py_list(prev),
                py_list(types)
            ));
        }
        Ok(())
    }

    // ---- expressions: (rust code, Kern type)
    fn expr(&mut self, e: &Expr, sc: &Scope) -> G<(String, String)> {
        match e {
            Expr::Int(n) => Ok((format!("{}i64", n), "Int".into())),
            // Rust's Debug of a str is a valid string literal (escapes included)
            Expr::Text(t) => Ok((format!("String::from({:?})", t), "Text".into())),
            Expr::Bin(op, l, r) => {
                if *op == "when" {
                    return unsupported("`when` outside a spec");
                }
                let (l, lt) = self.expr(l, sc)?;
                let (r, rt) = self.expr(r, sc)?;
                if *op == "+" && lt == "Text" && rt == "Text" {
                    // Text + Text joins them (wf typed it)
                    return Ok((format!("format!(\"{{}}{{}}\", {}, {})", l, r), "Text".into()));
                }
                Ok(match *op {
                    "+" => (format!("rt::add({}, {})?", l, r), "Int".into()),
                    "-" => (format!("rt::sub({}, {})?", l, r), "Int".into()),
                    "*" => (format!("rt::mul({}, {})?", l, r), "Int".into()),
                    "/" => (format!("rt::div({}, {})", l, r), "Int".into()),
                    "and" => (format!("({} && {})", l, r), "Bool".into()),
                    "or" => (format!("({} || {})", l, r), "Bool".into()),
                    "=" => (format!("({} == {})", l, r), "Bool".into()),
                    op => (format!("({} {} {})", l, op, r), "Bool".into()),
                })
            }
            Expr::Name(name, primed) => {
                if *primed {
                    return unsupported(format!("after-state read {}' outside a spec", name));
                }
                if let Some(t) = sc.get(name) {
                    let clone = if t == "Event" || t == "Text" { ".clone()" } else { "" };
                    return Ok((format!("{}{}", ident(name), clone), t.clone()));
                }
                self.apply(name, &[], sc)
            }
            Expr::App(name, primed, args) => {
                if *primed {
                    return unsupported(format!("after-state read {}' outside a spec", name));
                }
                self.apply(name, args, sc)
            }
            Expr::Sum(j, lo, hi, body, cond) => {
                // the interpreter's order: both bounds once, then per j in [lo, hi) the filter, then the body
                let (lo, _) = self.expr(lo, sc)?;
                let (hi, _) = self.expr(hi, sc)?;
                let mut inner = sc.clone();
                inner.insert(j.clone(), "Int".into());
                let (body, _) = self.expr(body, &inner)?;
                let add = format!("__s = rt::add(__s, {})?;", body);
                let step = match cond {
                    Some(c) => format!("if {} {{ {} }}", self.expr(c, &inner)?.0, add),
                    None => add,
                };
                Ok((
                    format!(
                        "{{ let __lo = {}; let __hi = {}; tx.spend(__lo, __hi)?; let mut __s = 0i64; let mut __j = __lo; \
                         while __j < __hi {{ let {} = __j; {} __j += 1; }} __s }}",
                        lo,
                        hi,
                        ident(j),
                        step
                    ),
                    "Int".into(),
                ))
            }
            Expr::All(..) => unsupported(format!("expression {}", code(e))),
            // new expression forms (e.g. a sum) are refused until the emitter compiles them: never mistranslated
            #[allow(unreachable_patterns)]
            _ => unsupported(format!("expression {}", code(e))),
        }
    }

    /// Arguments that call into the transaction are evaluated into temporaries first (left to right, as the
    /// interpreter does), so no argument borrows `tx` while the call holds it.
    fn hoist(fmt: &str, vals: &[(String, String)]) -> String {
        let codes: Vec<String> = vals.iter().map(|(c, _)| c.clone()).collect();
        if !codes.iter().any(|c| c.contains("tx")) {
            return fill(fmt, &codes);
        }
        let temps: Vec<String> = (0..codes.len()).map(|i| format!("__a{}", i)).collect();
        let lets: Vec<String> = temps.iter().zip(&codes).map(|(t, c)| format!("let {} = {};", t, c)).collect();
        format!("{{ {} {} }}", lets.join(" "), fill(fmt, &temps))
    }

    fn apply(&mut self, name: &str, args: &[Expr], sc: &Scope) -> G<(String, String)> {
        let mut vals = vec![];
        for a in args {
            vals.push(self.expr(a, sc)?);
        }
        let types: Vec<String> = vals.iter().map(|(_, t)| t.clone()).collect();
        let ph = vec!["{}"; vals.len()].join(", ");
        let sep = if vals.is_empty() { "" } else { ", " };
        if handler(name).is_some() {
            let fmt = match name {
                "ledger.balance" => "tx.balance({})",
                "ledger.owner" => "tx.owner({})",
                "ledger.total" => "tx.total()?",
                "ledger.debit" => "tx.debit({}, {})?",
                "ledger.credit" => "tx.credit({}, {})?",
                "clock.now" => "tx.now",
                "clock.tick" => "tx.tick()?",
                "audit.emit" => "tx.emit(0, {})",
                _ => unreachable!(),
            };
            return Ok((Self::hoist(fmt, &vals), sig(name).unwrap().1.to_string()));
        }
        if assumed_op(self.defs, name).is_some() {
            let fmt = format!("__h{}(tx{}{})?", ident(name), sep, ph);
            return Ok((Self::hoist(&fmt, &vals), "Int".into()));
        }
        if let Some(ch) = channel_op(self.defs, name) {
            let i = self.channels.iter().position(|c| c == ch).unwrap();
            return Ok((Self::hoist(&format!("tx.emit({}, {{}})", i), &vals), "Unit".into()));
        }
        if let Some((x, op, key)) = state_op(self.defs, name) {
            let pre = if key == "Account" { "a" } else { "i" };
            let (_, slot, text) = self.slot[x].clone();
            let (t, ty) = if text { ("t", "Text") } else { ("", "Int") };
            if op == "get" {
                return Ok((Self::hoist(&format!("tx.{}{}get({}, {{}})", pre, t, slot), &vals), ty.into()));
            }
            return Ok((Self::hoist(&format!("tx.{}{}set({}, {{}}, {{}})", pre, t, slot), &vals), "Unit".into()));
        }
        if name == "length" {
            return Ok((Self::hoist("rt::length(&{})", &vals), "Int".into()));
        }
        if let Some(d) = self.defs.defs.get(name) {
            let fmt = format!("{}(tx{}{})?", ident(name), sep, ph);
            // a record result's type is the record's name: only a let binds it (block), anything else won't type
            return Ok((Self::hoist(&fmt, &vals), d.ret.clone().unwrap_or_else(|| "Unit".into())));
        }
        if self.defs.records.contains_key(name) {
            // a record value is bound by a let (block) or set as `result`; nowhere else does it have a Rust form
            return unsupported(format!("record value {} outside `x = …` / `result = …`", name));
        }
        if name == "distinct" {
            let temps: Vec<String> = (0..vals.len()).map(|i| format!("__d{}", i)).collect();
            let lets: Vec<String> = temps.iter().zip(&vals).map(|(t, (c, _))| format!("let {} = {};", t, c)).collect();
            let mut pairs = vec![];
            for i in 0..temps.len() {
                for b in &temps[i + 1..] {
                    pairs.push(format!("{} != {}", temps[i], b));
                }
            }
            return Ok((format!("{{ {} {} }}", lets.join(" "), pairs.join(" && ")), "Bool".into()));
        }
        if name.chars().next().map_or(false, |c| c.is_uppercase()) {
            Self::ctor(&mut self.events, name, &types, &format!("event {}", name))?;
            if vals.is_empty() {
                return Ok((format!("Event::{}", variant(name)), "Event".into()));
            }
            let codes: Vec<&str> = vals.iter().map(|(c, _)| c.as_str()).collect();
            return Ok((format!("Event::{}({})", variant(name), codes.join(", ")), "Event".into()));
        }
        unsupported(format!("unknown name {}", name))
    }

    // ---- statements
    fn need(&mut self, c: &Expr, ctor: &str, args: &[Expr], sc: &Scope, ind: &str, d: &Def) -> G<Vec<String>> {
        let (cond, _) = self.expr(c, sc)?;
        let mut vals = vec![];
        for a in args {
            vals.push(self.expr(a, sc)?);
        }
        let types: Vec<String> = vals.iter().map(|(_, t)| t.clone()).collect();
        Self::ctor(&mut self.refusals, ctor, &types, &format!("{}: refusal {}", d.name, ctor))?;
        let payload = if vals.is_empty() {
            String::new()
        } else {
            format!("({})", vals.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join(", "))
        };
        Ok(vec![format!("{}if !({}) {{ return Err(Stop::Refused(Refusal::{}{})); }}", ind, cond, variant(ctor), payload)])
    }

    fn block(&mut self, stmts: &[Stmt], sc: &mut Scope, ind: &str, d: &Def) -> G<Vec<String>> {
        let mut out = vec![];
        let deeper = format!("{}    ", ind);
        for s in stmts {
            match s {
                Stmt::Need(c, ctor, args) => out.extend(self.need(c, ctor, args, sc, ind, d)?),
                Stmt::Let(x, e) if self.record_of(e).is_some() => out.extend(self.let_record(x, e, sc, ind, d)?),
                Stmt::Let(x, e) => {
                    let (c, t) = self.expr(e, sc)?;
                    if x == "result" && d.ret.is_some() {
                        out.push(format!("{}result = {};", ind, c));
                    } else {
                        if t == "Unit" {
                            return unsupported(format!("{}: let of a Unit value", d.name));
                        }
                        out.push(format!("{}let {} = {};", ind, ident(x), c));
                        sc.insert(x.clone(), t);
                    }
                }
                Stmt::If(c, t, o) => {
                    let (c, _) = self.expr(c, sc)?;
                    out.push(format!("{}if ({}) {{", ind, c));
                    out.extend(self.block(t, &mut sc.clone(), &deeper, d)?);
                    if !o.is_empty() {
                        out.push(format!("{}}} else {{", ind));
                        out.extend(self.block(o, &mut sc.clone(), &deeper, d)?);
                    }
                    out.push(format!("{}}}", ind));
                }
                Stmt::For(k, lo, hi, _, body) => {
                    // bounds evaluated once, charged against the call's iteration limit (S21); invariants are proven
                    let (lo, _) = self.expr(lo, sc)?;
                    let (hi, _) = self.expr(hi, sc)?;
                    out.push(format!("{}let __lo = {}; let __hi = {}; tx.spend(__lo, __hi)?;", ind, lo, hi));
                    out.push(format!("{}for {} in __lo..__hi {{", ind, ident(k)));
                    let mut inner = sc.clone();
                    inner.insert(k.clone(), "Int".into());
                    out.extend(self.block(body, &mut inner, &deeper, d)?);
                    out.push(format!("{}}}", ind));
                }
                Stmt::Do(e) => {
                    let (c, _) = self.expr(e, sc)?;
                    out.push(format!("{}{};", ind, c));
                }
                Stmt::Inv(_) => return unsupported(format!("{}: statement inv", d.name)),
            }
        }
        Ok(out)
    }

    /// A parameter or result type the runtime has no representation for (Text, until it does) is refused loudly,
    /// never compiled as something else.
    fn types_supported(&self, d: &Def, what: &str) -> G<()> {
        if let Some((p, t)) = d.params.iter().find(|(_, t)| rust_type(t).is_none()) {
            return unsupported(format!("{}: {} parameter {} of type {}", d.name, what, p, t));
        }
        match d.ret.as_deref() {
            None | Some("Int") => Ok(()),
            Some(r) if what == "definition" && self.leaves(r).map_or(false, |ls| ls.iter().all(|(_, t)| rust_type(t).is_some())) => Ok(()),
            Some(t) => unsupported(format!("{}: {} result of type {}", d.name, what, t)),
        }
    }

    /// A record type's leaf fields in order (`a.x`, `a.y`, `name`), as the interpreter's constructor value holds them.
    fn leaves(&self, r: &str) -> Option<Vec<(String, String)>> {
        self.defs.records.contains_key(r).then(|| crate::syntax::record_leaves(&self.defs.records, r))
    }

    /// The Rust type of a definition's result: i64, or a tuple of a record's leaves.
    fn ret_type(&self, d: &Def) -> String {
        match d.ret.as_deref() {
            None => "()".into(),
            Some("Int") => "i64".into(),
            Some(r) => format!("({},)", self.leaves(r).unwrap().iter().map(|(_, t)| rust_type(t).unwrap()).collect::<Vec<_>>().join(", ")),
        }
    }

    /// The record type a let's value has: a constructor of leaves (`Point a b`, the load-time form of `q = p` and
    /// `o = order.get k`) or a call to a definition returning one.
    fn record_of(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::App(n, false, _) if self.defs.records.contains_key(n) => Some(n.clone()),
            Expr::App(n, false, _) | Expr::Name(n, false) => {
                self.defs.defs.get(n).and_then(|f| f.ret.clone()).filter(|r| self.defs.records.contains_key(r))
            }
            _ => None,
        }
    }

    /// `x = <record>`: one Rust local per leaf (`x.a.x` is ident("x.a.x")), so `x.f` reads like any name; `result =
    /// <record>` sets the result tuple. Leaves evaluate left to right, as the interpreter builds the constructor.
    fn let_record(&mut self, x: &str, e: &Expr, sc: &mut Scope, ind: &str, d: &Def) -> G<Vec<String>> {
        let r = self.record_of(e).unwrap();
        let leaves = self.leaves(&r).unwrap();
        let is_result = x == "result" && d.ret.as_deref() == Some(r.as_str());
        if x == "result" && !is_result {
            return unsupported(format!("{}: result = a {} in a definition returning {:?}", d.name, r, d.ret));
        }
        let mut out = vec![];
        let names: Vec<String> = leaves.iter().map(|(f, _)| format!("{}.{}", x, f)).collect();
        match e {
            Expr::App(n, _, args) if *n == r => {
                if args.len() != leaves.len() {
                    return unsupported(format!("{}: {} built from {} values, it has {} leaves", d.name, r, args.len(), leaves.len()));
                }
                let mut codes = vec![];
                for (i, (a, (_, t))) in args.iter().zip(&leaves).enumerate() {
                    let (c, at) = self.expr(a, sc)?;
                    if at != *t {
                        return unsupported(format!("{}: leaf {} of {} is {}, given {}", d.name, leaves[i].0, r, t, at));
                    }
                    codes.push(c);
                }
                if is_result {
                    out.push(format!("{}result = ({},);", ind, codes.join(", ")));
                } else {
                    // through temporaries: a leaf may read a name this let is about to shadow
                    for (i, c) in codes.iter().enumerate() {
                        out.push(format!("{}let __l{} = {};", ind, i, c));
                    }
                    for (i, n) in names.iter().enumerate() {
                        out.push(format!("{}let {} = __l{};", ind, ident(n), i));
                    }
                }
            }
            _ => {
                let (c, _) = self.expr(e, sc)?;
                if is_result {
                    out.push(format!("{}result = {};", ind, c));
                } else {
                    let pat: Vec<String> = names.iter().map(|n| ident(n)).collect();
                    out.push(format!("{}let ({},) = {};", ind, pat.join(", "), c));
                }
            }
        }
        if !is_result {
            for (n, (_, t)) in names.iter().zip(&leaves) {
                sc.insert(n.clone(), t.clone());
            }
        }
        Ok(out)
    }

    fn definition(&mut self, d: &Def) -> G<Vec<String>> {
        self.types_supported(d, "definition")?;
        let mut sc: Scope = d.params.iter().cloned().collect();
        let params: String = d.params.iter().map(|(p, t)| format!(", {}: {}", ident(p), rust_type(t).unwrap())).collect();
        let ret = self.ret_type(d);
        let mut out = vec![
            format!("/// {}", d.src_lines["header"][0]),
            "#[inline]".to_string(),
            format!("pub fn {}(tx: &mut Tx<Event>{}) -> R<{}> {{", ident(&d.name), params, ret),
        ];
        // nesting depth, as the interpreter counts it (MAX_CALL_DEPTH); recursion is allowed when proven to end
        out.push("    tx.enter()?;".into());
        if d.ret.is_some() {
            out.push(format!("    let mut result: {};", self.ret_type(d)));
        }
        let mut stmts = vec![];
        for n in &d.needs {
            match n {
                Need::Need(c, k, a) => stmts.push(Stmt::Need(c.clone(), k.clone(), a.clone())),
                Need::As(..) => return unsupported(format!("{}: spec need need_as", d.name)),
            }
        }
        stmts.extend(d.body.iter().cloned());
        out.extend(self.block(&stmts, &mut sc, "    ", d)?);
        out.push("    tx.leave();".into());
        out.push(if d.ret.is_some() { "    Ok(result)" } else { "    Ok(())" }.into());
        out.push("}".into());
        Ok(out)
    }

    /// An assumed handler: check its needs, ask the provider, check its ensures (a broken assumption is a fault,
    /// never a refusal).
    fn handler(&mut self, h: &Def) -> G<Vec<String>> {
        self.types_supported(h, "handler")?;
        let mut sc: Scope = h.params.iter().cloned().collect();
        let params: String = h.params.iter().map(|(p, t)| format!(", {}: {}", ident(p), rust_type(t).unwrap())).collect();
        let vals: Vec<String> = h
            .params
            .iter()
            .map(|(p, t)| match t.as_str() {
                "Int" => format!("Val::Int({})", ident(p)),
                "Text" => format!("Val::Text({}.clone())", ident(p)),
                _ => format!("Val::Acct({})", ident(p)),
            })
            .collect();
        let vals = vals.join(", ");
        let mut out = vec![
            format!("/// {}", h.src_lines["header"][0]),
            format!("fn __h{}(tx: &mut Tx<Event>{}) -> R<i64> {{", ident(&h.name), params),
        ];
        for n in &h.needs {
            let Need::Need(c, _, _) = n else { continue };
            let (cc, _) = self.expr(c, &sc)?;
            let msg = json_str(&format!("need of {} not met: {}", h.name, code(c)));
            out.push(format!("    if !({}) {{ return Err(Fault::Error({}.into()).into()); }}", cc, msg));
        }
        out.push(format!("    let result = tx.provide({}, &[{}])?;", json_str(&h.name), vals));
        sc.insert("result".into(), "Int".into());
        for e in &h.ensures {
            let c = if let Expr::Bin("when", body, cond) = e {
                let (cond, _) = self.expr(cond, &sc)?;
                let (body, _) = self.expr(body, &sc)?;
                format!("(!{} || {})", cond, body)
            } else {
                self.expr(e, &sc)?.0
            };
            let msg = json_str(&format!("the provider of {} broke its assumed spec: {}", h.name, code(e)));
            out.push(format!(
                "    if !({}) {{ return Err(Fault::Error(format!(\"{{}} with result {{}}\", {}, result)).into()); }}",
                c, msg
            ));
        }
        out.push(format!("    tx.record({}, &[{}], result);", json_str(&h.name), vals));
        out.push("    Ok(result)".into());
        out.push("}".into());
        Ok(out)
    }

    // ---- the module
    fn show_arm(enum_: &str, name: &str, types: &[String]) -> String {
        if types.is_empty() {
            return format!("            {}::{} => {}.to_string(),", enum_, variant(name), json_str(name));
        }
        let vs: Vec<String> = (0..types.len()).map(|i| format!("x{}", i)).collect();
        let parts: Vec<String> = vs
            .iter()
            .zip(types)
            .map(|(v, t)| match t.as_str() {
                "Int" => format!("{}.to_string()", v),
                "Text" => format!("{}.clone()", v),
                "Account" => format!("w.name(*{}).to_string()", v),
                "Owner" => format!("w.owner_name(*{}).to_string()", v),
                _ => format!("rt::show_bool(*{}).to_string()", v),
            })
            .collect();
        format!(
            "            {}::{}({}) => [{}.to_string(), {}].join(\" \"),",
            enum_,
            variant(name),
            vs.join(", "),
            json_str(name),
            parts.join(", ")
        )
    }

    fn enum_(name: &str, table: &IndexMap<String, Vec<String>>) -> Vec<String> {
        if table.is_empty() {
            return vec!["#[derive(Clone, Debug, PartialEq)]".into(), format!("pub enum {} {{}}", name)];
        }
        let mut out = vec!["#[derive(Clone, Debug, PartialEq)]".to_string(), format!("pub enum {} {{", name)];
        for (c, types) in table {
            if types.is_empty() {
                out.push(format!("    {},", variant(c)));
            } else {
                let ts: Vec<&str> = types.iter().map(|t| rust_type(t).unwrap()).collect();
                out.push(format!("    {}({}),", variant(c), ts.join(", ")));
            }
        }
        out.push("}".into());
        out
    }

    pub fn module(&mut self) -> G<String> {
        let mut body = vec![];
        let defs = self.defs;
        for h in defs.handlers.values() {
            body.extend(self.handler(h)?);
            body.push(String::new());
        }
        for d in defs.defs.values() {
            body.extend(self.definition(d)?);
            body.push(String::new());
        }
        for (name, which) in self.check.clone() {
            body.extend(self.invariant_check(&name, &which)?);
            body.push(String::new());
        }
        let mut out: Vec<String> = vec![
            format!("// GENERATED by kern/emit_rust.py from {}.k. Do not edit: edit the .k file and rebuild.", self.modname),
            "// Semantics: kern/run.py (the reference); contract and divergences: docs/research/RUNTIME_PLAN.md.".into(),
            "#![allow(unused_imports, dead_code, unused_variables, unused_mut, unused_parens, unused_braces, unused_assignments, non_snake_case, non_camel_case_types, clippy::all)]".into(),
            "use kern_rt as rt;".into(),
            "use kern_rt::{Acct, DefInfo, Exec, Fault, KeyType, MapInfo, Owner, Provider, Stop, Tx, Val, World};".into(),
            String::new(),
        ];
        out.extend(Self::enum_("Refusal", &self.refusals));
        out.push(String::new());
        out.extend(Self::enum_("Event", &self.events));
        out.push(String::new());
        out.push("type R<T> = Result<T, Stop<Refusal>>;".into());
        out.push(String::new());
        out.extend(body);
        out.extend(
            [
                "pub struct M;",
                "",
                "impl rt::Module for M {",
                "    type Refusal = Refusal;",
                "    type Event = Event;",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        out.push(format!("    const NAME: &'static str = {};", json_str(&self.modname)));
        out.push("    const DEFS: &'static [DefInfo] = &[".into());
        for d in defs.defs.values() {
            let ps: Vec<String> = d
                .params
                .iter()
                .map(|(p, t)| format!("({}, KeyType::{})", json_str(p), match t.as_str() { "Account" => "Account", "Text" => "Text", _ => "Int" }))
                .collect();
            let us: Vec<String> = infer_uses(defs, d).iter().map(|u| json_str(u)).collect();
            out.push(format!(
                "        DefInfo {{ name: {}, hash: {}, params: &[{}], ret: {}, uses: &[{}] }},",
                json_str(&d.name),
                json_str(&d.hash()),
                ps.join(", "),
                d.ret.as_ref().map_or("None".to_string(), |r| format!("Some({})", json_str(r))),
                us.join(", ")
            ));
        }
        out.push("    ];".into());
        let maps: Vec<String> = self
            .maps
            .iter()
            .map(|(n, k, s, t)| format!("MapInfo {{ name: {}, key: KeyType::{}, slot: {}, text: {} }}", json_str(n), k, s, t))
            .collect();
        out.push(format!("    const MAPS: &'static [MapInfo] = &[{}];", maps.join(", ")));
        let chans: Vec<String> = self.channels.iter().map(|c| json_str(c)).collect();
        out.push(format!("    const CHANNELS: &'static [&'static str] = &[{}];", chans.join(", ")));
        out.push("    fn call(exec: &mut Exec<Event>, w: &mut World, prov: &mut dyn Provider, def: usize, args: &[Val]) -> R<Option<Vec<Val>>> {".into());
        out.push("        match def {".into());
        for (i, d) in defs.defs.values().enumerate() {
            let conv: Vec<String> = d
                .params
                .iter()
                .enumerate()
                .map(|(j, (_, t))| format!("rt::{}(&args[{}])", match t.as_str() { "Account" => "acct", "Text" => "text", _ => "int" }, j))
                .collect();
            // the result as the interpreter's value: an Int, or a record's leaves in order
            let wrap = match d.ret.as_deref() {
                None => ".map(|_| None)".to_string(),
                Some("Int") => ".map(|v| Some(vec![Val::Int(v)]))".to_string(),
                Some(r) => {
                    let ls = self.leaves(r).unwrap();
                    let pat: Vec<String> = (0..ls.len()).map(|i| format!("__{}", i)).collect();
                    let vals: Vec<String> = ls
                        .iter()
                        .enumerate()
                        .map(|(i, (_, t))| format!("Val::{}(__{})", match t.as_str() { "Account" => "Acct", "Text" => "Text", _ => "Int" }, i))
                        .collect();
                    format!(".map(|({},)| Some(vec![{}]))", pat.join(", "), vals.join(", "))
                }
            };
            let call = format!("{}(tx{}{})", ident(&d.name), if conv.is_empty() { "" } else { ", " }, conv.join(", "));
            let call = if self.check.contains_key(&d.name) {
                format!("{{ let __r = {}?; __inv_{}(tx)?; Ok(__r) }}", call, ident(&d.name))
            } else {
                call
            };
            out.push(format!("            {} => exec.call(w, prov, |tx| {}){},", i, call, wrap));
        }
        out.push("            _ => unreachable!(\"the driver checked the definition\"),".into());
        out.push("        }".into());
        out.push("    }".into());
        out.push("    fn show_refusal(r: &Refusal, w: &World) -> String {".into());
        out.push("        match r {".into());
        out.extend(self.refusals.iter().map(|(c, t)| Self::show_arm("Refusal", c, t)));
        if self.refusals.is_empty() {
            out.push("            _ => unreachable!(),".into());
        }
        out.push("        }".into());
        out.push("    }".into());
        out.push("    fn show_event(e: &Event, w: &World) -> String {".into());
        out.push("        match e {".into());
        out.extend(self.events.iter().map(|(c, t)| Self::show_arm("Event", c, t)));
        if self.events.is_empty() {
            out.push("            _ => unreachable!(),".into());
        }
        out.push("        }".into());
        out.push("    }".into());
        out.push("}".into());
        Ok(out.join("\n") + "\n")
    }
}

fn py_list(xs: &[String]) -> String {
    format!("[{}]", xs.iter().map(|x| crate::pyrepr::repr_str(x)).collect::<Vec<_>>().join(", "))
}

/// The file's name, or the package directory's name for any file of a package.
pub fn crate_name(path: &str) -> String {
    let base = root_of(path).unwrap_or_else(|| abspath(path));
    let file = base.rsplit('/').next().unwrap_or("");
    let stem = match file.trim_start_matches('.').rfind('.') {
        Some(i) => &file[..i + (file.len() - file.trim_start_matches('.').len())],
        None => file,
    };
    stem.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// The repository this binary was built from, when it is still there (a checkout, or the Docker build tree).
fn source_root() -> Option<String> {
    let root = abspath(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    Path::new(&format!("{}/runtime/Cargo.toml", root)).exists().then_some(root)
}

/// Where generated crates and cargo's target go: KERN_BUILD, else the checkout's build/, else ./build.
pub fn build_dir() -> String {
    if let Ok(d) = std::env::var("KERN_BUILD") {
        return abspath(&d);
    }
    match source_root() {
        Some(r) => format!("{}/build", r),
        None => abspath("build"),
    }
}

const RT_CARGO: &str = include_str!("../../runtime/Cargo.toml");
const RT_LIB: &str = include_str!("../../runtime/src/lib.rs");
const RT_DRIVER: &str = include_str!("../../runtime/src/driver.rs");

/// The kern-rt crate to link with: KERN_RUNTIME, else the checkout's runtime/, else the copy built into this binary,
/// written out under the build directory (keyed by its content, so an upgrade never links a stale runtime).
pub fn runtime_dir() -> std::io::Result<String> {
    if let Ok(d) = std::env::var("KERN_RUNTIME") {
        return Ok(abspath(&d));
    }
    if let Some(r) = source_root() {
        return Ok(format!("{}/runtime", r));
    }
    embedded_runtime(&build_dir())
}

/// Write the kern-rt copy built into this binary under `build` (once per content) and return its directory.
pub fn embedded_runtime(build: &str) -> std::io::Result<String> {
    let key = crate::pyrepr::sha_hex(&format!("{}\0{}\0{}", RT_CARGO, RT_LIB, RT_DRIVER), 12);
    let dir = format!("{}/kern-rt-{}", build, key);
    if !Path::new(&format!("{}/src/driver.rs", dir)).exists() {
        std::fs::create_dir_all(format!("{}/src", dir))?;
        std::fs::write(format!("{}/Cargo.toml", dir), RT_CARGO)?;
        std::fs::write(format!("{}/src/lib.rs", dir), RT_LIB)?;
        std::fs::write(format!("{}/src/driver.rs", dir), RT_DRIVER)?;
    }
    Ok(dir)
}

/// The generated crate's files: (relative path, text).
pub fn crate_files(defs: &Module, name: &str, runtime: &str, check: IndexMap<String, Vec<usize>>) -> G<Vec<(String, String)>> {
    let src = Gen::new(defs, name).checking(check).module()?;
    let cargo = format!(
        "# GENERATED by kern/emit_rust.py\n[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nkern-rt = {{ path = \"{}\" }}\n\n[profile.release]\nopt-level = 3\ncodegen-units = 1\n\n\
         [workspace]\n",
        name, runtime
    );
    Ok(vec![
        ("Cargo.toml".into(), cargo),
        (
            "src/main.rs".into(),
            "// GENERATED by kern/emit_rust.py\nmod module;\n\nfn main() {\n    kern_rt::driver::main::<module::M>()\n}\n".into(),
        ),
        ("src/module.rs".into(), src),
    ])
}

/// definition -> the module invariants its compiled calls must check: each one with a goal for that definition
/// (the invariant itself, or a loop's "module invariant" goal) that is not ✔ (analyse without mutation). An invariant
/// with no goal reads nothing the definition writes (the inferred frame), so the call keeps it.
pub fn unproven_invariants(defs: &Module) -> Result<IndexMap<String, Vec<usize>>, BuildError> {
    if defs.invariants.is_empty() {
        return Ok(IndexMap::new());
    }
    let reports = crate::report::analyse(defs, false, None, None).map_err(|e| BuildError::Runtime(e.message()))?;
    let mut out = IndexMap::new();
    for (n, r) in &reports {
        let which: Vec<usize> = defs
            .invariants
            .iter()
            .enumerate()
            .filter(|(_, (var, t, cond))| {
                let text = format!("{}:{} {}", var, t, code(cond));
                r.guarantees.iter().any(|g| {
                    let mine = (g.kind == "invariant" && g.clause == text)
                        || (g.kind == "loop" && g.clause.starts_with(&format!("module invariant {}", text)));
                    mine && g.status != crate::report::Status::Proven
                })
            })
            .map(|(i, _)| i)
            .collect();
        if !which.is_empty() {
            out.insert(n.clone(), which);
        }
    }
    Ok(out)
}

/// Generate the crate for module `path`; returns its directory.
pub fn emit(path: &str, out_dir: Option<&str>) -> Result<String, BuildError> {
    let name = crate_name(path);
    let defs = load(path, &IndexMap::new())?;
    let runtime = runtime_dir().map_err(|e| BuildError::Runtime(format!("cannot write the runtime crate: {}", e)))?;
    let check = unproven_invariants(&defs)?;
    let files = crate_files(&defs, &name, &runtime, check)?;
    let out_dir = match out_dir {
        Some(d) => d.to_string(),
        None => format!("{}/{}", build_dir(), name),
    };
    let io = |e: std::io::Error| BuildError::Runtime(format!("{}: {}", out_dir, e));
    std::fs::create_dir_all(format!("{}/src", out_dir)).map_err(io)?;
    for (rel, text) in files {
        let p = format!("{}/{}", out_dir, rel);
        // keep mtimes stable: cargo rebuilds less
        if std::fs::read_to_string(&p).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&p, text).map_err(io)?;
        }
    }
    Ok(out_dir)
}

/// cargo build --release; returns the binary path. Warnings are errors (scorecard C1).
pub fn build(crate_dir: &str) -> Result<String, BuildError> {
    let target = format!("{}/target", build_dir());
    let r = std::process::Command::new("cargo")
        .args(["build", "--release", "-q"])
        .current_dir(crate_dir)
        .env("CARGO_TARGET_DIR", &target)
        .env("RUSTFLAGS", "-D warnings")
        .output()
        .map_err(|e| BuildError::Runtime(format!("cargo build failed in {}: {} (is cargo installed?)", crate_dir, e)))?;
    if !r.status.success() {
        return Err(BuildError::Runtime(format!(
            "cargo build failed in {}:\n{}",
            crate_dir,
            String::from_utf8_lossy(&r.stderr)
        )));
    }
    // the binary is named after the package, which is the module's name, not the directory's (`--out DIR`)
    let cargo = std::fs::read_to_string(format!("{}/Cargo.toml", crate_dir)).unwrap_or_default();
    let base = cargo
        .lines()
        .find_map(|l| l.strip_prefix("name = \"").and_then(|r| r.strip_suffix('"')))
        .map(str::to_string)
        .unwrap_or_else(|| crate_dir.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string());
    Ok(format!("{}/release/{}", target, base))
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    /// ✗ on `mark` (a negative flag), ○ on `put` (a computed key), ✔ on `safe`; `note` writes nothing an invariant
    /// reads, so it has no invariant goal at all.
    const HOLDS: &str = "state hold Int
state flag Account
state seen Account
invariant o:Int hold.get o >= 0
invariant a:Account flag.get a >= 0 and ledger.balance a >= 0

put k:Int v:Int
  need v >= 0 else Neg
body
  hold.set (k + 1) (v - 5)

mark a:Account v:Int
  ensure flag.get' a = v
body
  flag.set a v

safe a:Account
  ensure flag.get' a = 1
body
  flag.set a 1

note a:Account
  ensure seen.get' a = 1
body
  seen.set a 1
";

    #[test]
    fn only_unproven_invariants_get_a_runtime_check() {
        let defs = crate::syntax::parse_module(HOLDS, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let check = super::unproven_invariants(&defs).unwrap();
        assert_eq!(check, IndexMap::from([("put".to_string(), vec![0]), ("mark".to_string(), vec![1])]));
        let src = super::Gen::new(&defs, "holds").checking(check).module().unwrap();
        assert!(src.contains("{ let __r = put(tx, rt::int(&args[0]), rt::int(&args[1]))?; __inv_put(tx)?; Ok(__r) }"), "{src}");
        assert!(src.contains("2 => exec.call(w, prov, |tx| safe(tx, rt::acct(&args[0]))).map(|_| None),"));
        assert!(src.contains("3 => exec.call(w, prov, |tx| note(tx, rt::acct(&args[0]))).map(|_| None),"));
        assert!(src.contains("for o in tx.written_ints() {") && src.contains("for a in tx.written_accts() {"));
        assert!(!src.contains("fn __inv_safe") && !src.contains("fn __inv_note"));
    }

    #[test]
    fn identifiers_are_injective() {
        let names = [
            "p.x", "p_x", "p__x", "p_.x", "p._x", "a_b.c", "a.b_c", "a.b.c", "rand.below", "rand_below", "tx", "tx_", "w", "w_",
            "move", "move_", "self", "r", "a0", "inv__put", "h__rand_below", "_w", "__a0",
        ];
        let ids: Vec<String> = names.iter().map(|n| super::ident(n)).collect();
        let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), names.len(), "{ids:?}");
        assert_eq!((ids[0].as_str(), ids[1].as_str(), ids[12].as_str(), ids[13].as_str()), ("_p_0x", "p_x", "_w", "w_"));
        // the generator's own names are never an identifier of a Kern name
        for own in ["tx", "rt", "w", "exec", "prov", "args", "__a0", "__d1", "__r", "__h_rand_0below", "__inv_put", "__lo", "__hi", "__s", "__j"] {
            assert!(!ids.contains(&own.to_string()), "{own}");
        }
    }

    #[test]
    fn a_type_the_runtime_lacks_is_refused_not_mistranslated() {
        let src = "state hold Int\n\nput k:Int\nbody\n  hold.set k 1\n\nget k:Int -> Int\nbody\n  result = k\n";
        let base = crate::syntax::parse_module(src, true).unwrap();
        let mut a = base.clone();
        a.defs.get_mut("put").unwrap().params[0].1 = "List".into();
        let e = super::Gen::new(&a, "t").module().unwrap_err().0;
        assert!(e.contains("put: definition parameter k of type List"), "{e}");
        let mut b = base.clone();
        b.defs.get_mut("get").unwrap().ret = Some("Text".into());
        let e = super::Gen::new(&b, "t").module().unwrap_err().0;
        assert!(e.contains("get: definition result of type Text"), "{e}");
        assert!(super::Gen::new(&base, "t").module().is_ok());
    }

    #[test]
    fn a_record_result_is_a_tuple_of_its_leaves_and_a_bound_record_one_local_per_leaf() {
        let src = "record Point x:Int y:Int\nrecord Tag n:Int t:Text p:Point\n\nmid p:Point q:Point -> Point\nbody\n  \
                   result = Point (p.x + q.x) (p.y + q.y)\n\ntag n:Int -> Tag\nbody\n  m = mid (Point n n) (Point 1 2)\n  \
                   result = Tag m.x \"a\" m\n";
        let defs = crate::syntax::parse_module(src, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let rs = super::Gen::new(&defs, "t").module().unwrap();
        let has = |s: &str| assert!(rs.contains(s), "missing {s:?} in\n{rs}");
        has("-> R<(i64, i64,)> {");
        has("let mut result: (i64, String, i64, i64,);");
        has("let (_m_0x, _m_0y,) = mid(tx, ");
        has("result = (_m_0x, String::from(\"a\"), _m_0x, _m_0y,);");
        has(".map(|(__0, __1, __2, __3,)| Some(vec![Val::Int(__0), Val::Text(__1), Val::Int(__2), Val::Int(__3)]))");
        has("ret: Some(\"Tag\")");
        // a record type the runtime can't hold is still refused, not mistranslated
        let mut b = defs.clone();
        b.records.get_mut("Point").unwrap()[0].1 = "List".into();
        assert!(super::Gen::new(&b, "t").module().unwrap_err().0.contains("result of type"));
    }

    #[test]
    fn the_runtime_nests_calls_as_deep_as_the_interpreter() {
        assert!(super::RT_LIB.contains(&format!("pub const MAX_CALL_DEPTH: u32 = {};", crate::run::MAX_CALL_DEPTH)));
    }

    #[test]
    fn a_proven_module_compiles_exactly_as_before() {
        // every invariant goal of limits.k is proven: no check, the same text as without the analysis
        let defs = crate::package::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/limits/limits.k"), &IndexMap::new()).unwrap();
        let check = super::unproven_invariants(&defs).unwrap();
        assert!(check.is_empty(), "{check:?}");
        assert_eq!(super::Gen::new(&defs, "limits").checking(check).module(), super::Gen::new(&defs, "limits").module());
    }

    #[test]
    fn the_embedded_runtime_is_the_checkout_runtime() {
        let tmp = std::env::temp_dir().join(format!("kern-rt-test-{}", std::process::id()));
        let dir = super::embedded_runtime(&tmp.to_string_lossy()).unwrap();
        let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime");
        for f in ["Cargo.toml", "src/lib.rs", "src/driver.rs"] {
            assert_eq!(std::fs::read(format!("{dir}/{f}")).unwrap(), std::fs::read(format!("{repo}/{f}")).unwrap(), "{f}");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
