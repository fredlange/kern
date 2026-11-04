//! Kern -> Rust (port of kern/emit_rust.py). Turns a checked Kern module into a Rust crate that links with the
//! runtime crate kern-rt (runtime/).
//!
//!     kern build FILE.k [--out DIR]      (a file, a package or its directory; crates and cargo's target go in build/)
//!
//! Each definition becomes one Rust function over a `Tx` (the runtime's call in progress). `need`s are behaviour and
//! are compiled; `keep`/`ensure`/`emit` are proven, so they compile to nothing. The semantics are the interpreter's
//! (docs/research/RUNTIME_PLAN.md has the contract); validation/difftest.py checks the generated program against it.
//!
//! The generated text is byte-identical to Python's (toolchain/tests/emit_oracle.rs), so every existing build hash
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
        _ => return None,
    })
}

pub fn ident(name: &str) -> String {
    let name = name.replace('.', "_");
    let reserved = RUST_KEYWORDS.split_whitespace().any(|k| k == name)
        || ["tx", "rt", "w", "exec", "prov", "args"].contains(&name.as_str());
    if reserved {
        format!("{}_", name)
    } else {
        name
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
    maps: Vec<(String, String, usize)>,
    slot: IndexMap<String, (String, usize)>,
}

impl<'a> Gen<'a> {
    pub fn new(defs: &'a Module, modname: &str) -> Self {
        let mut channels = vec!["audit".to_string()];
        channels.extend(defs.channels.iter().cloned());
        let (mut maps, mut slots) = (vec![], IndexMap::from([("Account".to_string(), 0usize), ("Int".to_string(), 0)]));
        for (name, key) in &defs.states {
            let s = slots[key];
            maps.push((name.clone(), key.clone(), s));
            *slots.get_mut(key).unwrap() += 1;
        }
        let slot = maps.iter().map(|(n, k, s)| (n.clone(), (k.clone(), *s))).collect();
        Gen { defs, modname: modname.into(), refusals: IndexMap::new(), events: IndexMap::new(), channels, maps, slot }
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
            Expr::Bin(op, l, r) => {
                if *op == "when" {
                    return unsupported("`when` outside a spec");
                }
                let (l, _) = self.expr(l, sc)?;
                let (r, _) = self.expr(r, sc)?;
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
                    let clone = if t == "Event" { ".clone()" } else { "" };
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
            Expr::All(..) => unsupported(format!("expression {}", code(e))),
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
                "audit.emit" => "tx.emit(0, {})",
                _ => unreachable!(),
            };
            return Ok((Self::hoist(fmt, &vals), sig(name).unwrap().1.to_string()));
        }
        if assumed_op(self.defs, name).is_some() {
            let fmt = format!("h__{}(tx{}{})?", ident(name), sep, ph);
            return Ok((Self::hoist(&fmt, &vals), "Int".into()));
        }
        if let Some(ch) = channel_op(self.defs, name) {
            let i = self.channels.iter().position(|c| c == ch).unwrap();
            return Ok((Self::hoist(&format!("tx.emit({}, {{}})", i), &vals), "Unit".into()));
        }
        if let Some((x, op, key)) = state_op(self.defs, name) {
            let pre = if key == "Account" { "a" } else { "i" };
            let slot = self.slot[x].1;
            if op == "get" {
                return Ok((Self::hoist(&format!("tx.{}get({}, {{}})", pre, slot), &vals), "Int".into()));
            }
            return Ok((Self::hoist(&format!("tx.{}set({}, {{}}, {{}})", pre, slot), &vals), "Unit".into()));
        }
        if let Some(d) = self.defs.defs.get(name) {
            let fmt = format!("{}(tx{}{})?", ident(name), sep, ph);
            return Ok((Self::hoist(&fmt, &vals), if d.ret.is_some() { "Int" } else { "Unit" }.into()));
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
                return Ok((format!("Event::{}", name), "Event".into()));
            }
            let codes: Vec<&str> = vals.iter().map(|(c, _)| c.as_str()).collect();
            return Ok((format!("Event::{}({})", name, codes.join(", ")), "Event".into()));
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
        Ok(vec![format!("{}if !({}) {{ return Err(Stop::Refused(Refusal::{}{})); }}", ind, cond, ctor, payload)])
    }

    fn block(&mut self, stmts: &[Stmt], sc: &mut Scope, ind: &str, d: &Def) -> G<Vec<String>> {
        let mut out = vec![];
        let deeper = format!("{}    ", ind);
        for s in stmts {
            match s {
                Stmt::Need(c, ctor, args) => out.extend(self.need(c, ctor, args, sc, ind, d)?),
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
                    // bounds evaluated once; invariants are proven
                    let (lo, _) = self.expr(lo, sc)?;
                    let (hi, _) = self.expr(hi, sc)?;
                    out.push(format!("{}for {} in ({})..({}) {{", ind, ident(k), lo, hi));
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

    fn definition(&mut self, d: &Def) -> G<Vec<String>> {
        let mut sc: Scope = d.params.iter().cloned().collect();
        let params: String = d.params.iter().map(|(p, t)| format!(", {}: {}", ident(p), rust_type(t).unwrap())).collect();
        let ret = if d.ret.is_some() { "i64" } else { "()" };
        let mut out = vec![
            format!("/// {}", d.src_lines["header"][0]),
            "#[inline]".to_string(),
            format!("pub fn {}(tx: &mut Tx<Event>{}) -> R<{}> {{", ident(&d.name), params, ret),
        ];
        if d.ret.is_some() {
            out.push("    let mut result: i64;".into());
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
        out.push(if d.ret.is_some() { "    Ok(result)" } else { "    Ok(())" }.into());
        out.push("}".into());
        Ok(out)
    }

    /// An assumed handler: check its needs, ask the provider, check its ensures (a broken assumption is a fault,
    /// never a refusal).
    fn handler(&mut self, h: &Def) -> G<Vec<String>> {
        let mut sc: Scope = h.params.iter().cloned().collect();
        let params: String = h.params.iter().map(|(p, t)| format!(", {}: {}", ident(p), rust_type(t).unwrap())).collect();
        let vals: Vec<String> = h
            .params
            .iter()
            .map(|(p, t)| if t == "Int" { format!("Val::Int({})", ident(p)) } else { format!("Val::Acct({})", ident(p)) })
            .collect();
        let vals = vals.join(", ");
        let mut out = vec![
            format!("/// {}", h.src_lines["header"][0]),
            format!("fn h__{}(tx: &mut Tx<Event>{}) -> R<i64> {{", ident(&h.name), params),
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
            return format!("            {}::{} => {}.to_string(),", enum_, name, json_str(name));
        }
        let vs: Vec<String> = (0..types.len()).map(|i| format!("x{}", i)).collect();
        let parts: Vec<String> = vs
            .iter()
            .zip(types)
            .map(|(v, t)| match t.as_str() {
                "Int" => format!("{}.to_string()", v),
                "Account" => format!("w.name(*{}).to_string()", v),
                "Owner" => format!("w.owner_name(*{}).to_string()", v),
                _ => format!("rt::show_bool(*{}).to_string()", v),
            })
            .collect();
        format!(
            "            {}::{}({}) => [{}.to_string(), {}].join(\" \"),",
            enum_,
            name,
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
                out.push(format!("    {},", c));
            } else {
                let ts: Vec<&str> = types.iter().map(|t| rust_type(t).unwrap()).collect();
                out.push(format!("    {}({}),", c, ts.join(", ")));
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
        let mut out: Vec<String> = vec![
            format!("// GENERATED by kern/emit_rust.py from {}.k. Do not edit: edit the .k file and rebuild.", self.modname),
            "// Semantics: kern/run.py (the reference); contract and divergences: docs/research/RUNTIME_PLAN.md.".into(),
            "#![allow(unused_imports, dead_code, unused_variables, unused_mut, unused_parens, unused_braces, unused_assignments, non_snake_case, clippy::all)]".into(),
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
                .map(|(p, t)| format!("({}, KeyType::{})", json_str(p), if t == "Account" { "Account" } else { "Int" }))
                .collect();
            let us: Vec<String> = infer_uses(defs, d).iter().map(|u| json_str(u)).collect();
            out.push(format!(
                "        DefInfo {{ name: {}, hash: {}, params: &[{}], ret: {}, uses: &[{}] }},",
                json_str(&d.name),
                json_str(&d.hash()),
                ps.join(", "),
                if d.ret.is_some() { "true" } else { "false" },
                us.join(", ")
            ));
        }
        out.push("    ];".into());
        let maps: Vec<String> = self
            .maps
            .iter()
            .map(|(n, k, s)| format!("MapInfo {{ name: {}, key: KeyType::{}, slot: {} }}", json_str(n), k, s))
            .collect();
        out.push(format!("    const MAPS: &'static [MapInfo] = &[{}];", maps.join(", ")));
        let chans: Vec<String> = self.channels.iter().map(|c| json_str(c)).collect();
        out.push(format!("    const CHANNELS: &'static [&'static str] = &[{}];", chans.join(", ")));
        out.push("    fn call(exec: &mut Exec<Event>, w: &mut World, prov: &mut dyn Provider, def: usize, args: &[Val]) -> R<Option<i64>> {".into());
        out.push("        match def {".into());
        for (i, d) in defs.defs.values().enumerate() {
            let conv: Vec<String> = d
                .params
                .iter()
                .enumerate()
                .map(|(j, (_, t))| format!("rt::{}(args[{}])", if t == "Account" { "acct" } else { "int" }, j))
                .collect();
            let wrap = if d.ret.is_some() { ".map(Some)" } else { ".map(|_| None)" };
            out.push(format!(
                "            {} => exec.call(w, prov, |tx| {}(tx{}{}){}),",
                i,
                ident(&d.name),
                if conv.is_empty() { "" } else { ", " },
                conv.join(", "),
                wrap
            ));
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
pub fn crate_files(defs: &Module, name: &str, runtime: &str) -> G<Vec<(String, String)>> {
    let src = Gen::new(defs, name).module()?;
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

/// Generate the crate for module `path`; returns its directory.
pub fn emit(path: &str, out_dir: Option<&str>) -> Result<String, BuildError> {
    let name = crate_name(path);
    let defs = load(path, &IndexMap::new())?;
    let runtime = runtime_dir().map_err(|e| BuildError::Runtime(format!("cannot write the runtime crate: {}", e)))?;
    let files = crate_files(&defs, &name, &runtime)?;
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
    let base = crate_dir.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    Ok(format!("{}/release/{}", target, base))
}

#[cfg(test)]
mod tests {
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
