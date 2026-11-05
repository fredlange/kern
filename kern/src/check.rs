//! Examples, property tests and mutation testing (port of kern/check.py). The random streams are Python's
//! (pyrandom.rs), drawn in Python's order, so the counts and violations match the Python toolchain's.

use indexmap::IndexMap;

use crate::infer::{relevant_states, uses};
use crate::prove::Prover;
use crate::pyrandom::Random;
use crate::pyrepr::Py;
use crate::report::{ExampleResult, Mutation, Props, Status, Violation};
use crate::run::{Interp, Outcome as RunOutcome, RunError, Value, World};
use crate::syntax::{code, stmt_code, Def, Expr, Key, Module, Outcome, Stmt};
use crate::wf::problems;
use crate::Int;

/// An exception Python lets through from the interpreter (anything but KernError, KeyError, TypeError):
/// (type name, message). mutation_score reports it as "TYPE: MESSAGE".
#[derive(Clone, Debug)]
pub struct Crash(pub String, pub String);

fn caught(e: &RunError) -> bool {
    matches!(e, RunError::Kern(_) | RunError::Key(_) | RunError::Type(_))
}

fn crash(e: RunError) -> Crash {
    match e {
        RunError::Other(t, m) => Crash(t, m),
        RunError::Kern(m) => Crash("KernError".into(), m),
        RunError::Key(m) => Crash("KeyError".into(), m),
        RunError::Type(m) => Crash("TypeError".into(), m),
    }
}

fn key_value(k: &Key) -> Value {
    match k {
        Key::Int(n) => Value::Int(*n),
        Key::Str(s) => Value::Str(s.clone()),
    }
}

fn key_text(k: &Key) -> String {
    match k {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => s.clone(),
    }
}

fn args_text(args: &[Value]) -> String {
    args.iter().map(Value::str).collect::<Vec<_>>().join(" ")
}

/// d's `eg` lines, run in one interpreter (as Python does: its provider stream carries over).
pub fn run_examples(defs: &Module, d: &Def) -> Result<Vec<ExampleResult>, Crash> {
    let mut interp = Interp::new(defs);
    let mut out = vec![];
    for ex in &d.examples {
        let mut w = World::from_example(&ex.world, ex.now, &ex.maps);
        let args: Vec<Value> = ex.call.1.iter().map(key_value).collect();
        let res = interp.run(&ex.call.0, &args, &mut w);
        let outcome = match res {
            Ok(o) => o,
            Err(e) if caught(&e) => {
                out.push(ExampleResult { src: ex.src.clone(), pass: false, got: format!("error: {e}") });
                continue;
            }
            Err(e) => return Err(crash(e)),
        };
        let same = match (&outcome, &ex.outcome) {
            (RunOutcome::Ok, Outcome::Ok) => true,
            (RunOutcome::Fail(c, xs), Outcome::Fail(c2, ys)) => c == c2 && same_args(xs, ys),
            _ => false,
        };
        // Python's `and` chain: balances are read only when the outcome matched, in the eg's order, and
        // `after.bal[a]` raises KeyError (caught) for an account the world doesn't have
        let mut ok = same;
        let mut missing = None;
        if ok {
            for (a, b) in &ex.after {
                match w.bal.get(a.as_str()) {
                    None => {
                        missing = Some(a.clone());
                        break;
                    }
                    Some(v) if v != b => {
                        ok = false;
                        break;
                    }
                    _ => {}
                }
            }
        }
        if let Some(a) = missing {
            out.push(ExampleResult { src: ex.src.clone(), pass: false, got: format!("error: '{a}'") });
            continue;
        }
        ok = ok
            && ex.after_maps.iter().all(|(x, kv)| kv.iter().all(|(k, v)| w.maps.get(x).and_then(|m| m.get(k)).copied().unwrap_or(0) == *v));
        out.push(ExampleResult { src: ex.src.clone(), pass: ok, got: outcome.text() });
    }
    Ok(out)
}

/// Python `list(outcome[1]) == ex.outcome[1]`: the refusal payload equals the eg's ints.
fn same_args(xs: &[Value], ys: &[Int]) -> bool {
    xs.len() == ys.len()
        && xs.iter().zip(ys).all(|(x, y)| match x {
            Value::Int(n) => n == y,
            Value::Bool(b) => (*b as Int) == *y, // Python: True == 1
            _ => false,
        })
}

fn rand_args(d: &Def, accts: &[&str], rnd: &mut Random) -> Vec<Value> {
    let mut args = vec![];
    for (_, t) in &d.params {
        if t == "Account" {
            args.push(Value::Str(rnd.choice(accts).to_string()));
        } else {
            let opts = [rnd.randint(-5, 5), rnd.randint(0, 200), rnd.randint(0, 1200)];
            args.push(Value::Int(*rnd.choice(&opts) as Int));
        }
    }
    args
}

fn world_text(before: &World, accts: &[&str]) -> String {
    let mut s = accts.iter().map(|a| format!("{}:{}@{}", a, before.bal[*a], before.own[*a])).collect::<Vec<_>>().join(" ");
    s += &format!(" now:{}", before.clock);
    s
}

/// Random worlds and arguments; every success is checked against d's guarantees and the module invariants.
pub fn proptest(defs: &Module, d: &Def, n: u64, seed: i128) -> Result<Props, Crash> {
    let mut rnd = Random::from_int(seed);
    let mut interp = Interp::new(defs);
    let mut times = Random::from_int(seed + 1_000_003);
    let mut per_state: IndexMap<String, Random> = IndexMap::new();
    let mut stats = Props { runs: n, succeeded: 0, failures: IndexMap::new(), violations: vec![] };
    let touched = uses(defs, d);
    let states_used = relevant_states(defs, d);
    let checks_invariants = touched.iter().any(|e| e.ends_with(".write") || e == "ledger.write");
    let accts = ["A", "B", "C"];
    for _ in 0..n {
        let bal: IndexMap<String, Int> = accts.iter().map(|a| (a.to_string(), rnd.randint(0, 1000) as Int)).collect();
        let own: IndexMap<String, String> = accts.iter().map(|a| (a.to_string(), rnd.choice(&["u1", "u2"]).to_string())).collect();
        let mut w = World::new(bal, own, times.randint(0, 100) as Int);
        for (x, key_type) in &states_used {
            let keys: Vec<Key> = if key_type == "Account" {
                accts.iter().map(|a| Key::Str(a.to_string())).collect()
            } else {
                (-5..6).map(|k| Key::Int(k)).collect()
            };
            let r = per_state.entry(x.clone()).or_insert_with(|| Random::from_str(&format!("{seed}:{x}")));
            let mut m = IndexMap::new();
            for k in keys {
                let big = r.randint(0, 300) as Int; // the list [0, 0, 1, randint] is built before the choice
                let v = *r.choice(&[0, 0, 1, big]);
                m.insert(k, v);
            }
            w.maps.insert(x.clone(), m);
        }
        let args = rand_args(d, &accts, &mut rnd);
        let before = w.clone();
        let outcome = match interp.run(&d.name, &args, &mut w) {
            Ok(o) => o,
            Err(e) if caught(&e) => {
                stats.violations.push(Violation::Error { error: e.to_string(), args: args.iter().map(value_py).collect() });
                continue;
            }
            Err(e) => return Err(crash(e)),
        };
        let mut after = w;
        match &outcome {
            RunOutcome::Ok => {
                stats.succeeded += 1;
                let mut broken = vec![];
                if checks_invariants {
                    let mut b = before.clone();
                    if interp.broken_invariants(&mut b, None).map_err(crash)?.is_empty() {
                        broken = interp
                            .broken_invariants(&mut after, None)
                            .map_err(crash)?
                            .into_iter()
                            .map(|(text, _)| ("invariant".to_string(), text))
                            .collect();
                    }
                }
                let mut pre = before.clone();
                let mut held = interp.holds(d, &args, &mut pre, &mut after).map_err(crash)?;
                held.extend(broken);
                for (kind, c) in held {
                    if stats.violations.len() < 3 {
                        let mut world = world_text(&before, &accts);
                        for (x, kv) in &before.maps {
                            for (k, v) in kv {
                                if *v != 0 {
                                    world += &format!(" {}.{}:{}", x, key_text(k), v);
                                }
                            }
                        }
                        stats.violations.push(Violation::Clause {
                            clause: format!("{kind} {c}"),
                            eg: format!("eg {} | {} {}", world, d.name, args_text(&args)),
                        });
                    }
                }
            }
            RunOutcome::Fail(ctor, _) => {
                *stats.failures.entry(ctor.clone()).or_insert(0) += 1;
                if interp.unstated_refusal(d, &args, &before, &outcome).map_err(crash)? && stats.violations.len() < 3 {
                    let world = world_text(&before, &accts);
                    stats.violations.push(Violation::Clause {
                        clause: "ok otherwise".into(),
                        eg: format!("eg {} | {} {} -> {}", world, d.name, args_text(&args), outcome.text()),
                    });
                }
            }
        }
    }
    Ok(stats)
}

fn value_py(v: &Value) -> Py {
    match v {
        Value::Int(n) => Py::Int(*n),
        Value::Str(s) => Py::Str(s.clone()),
        Value::Bool(b) => Py::Bool(*b),
        Value::None => Py::None,
        Value::Ctor(c, a) => Py::Tuple(std::iter::once(Py::Str(c.clone())).chain(a.iter().map(value_py)).collect()),
    }
}

// ---------- mutation ----------

fn swap(op: &str) -> Option<&'static str> {
    Some(match op {
        "+" => "-",
        "-" => "+",
        "*" => "/",
        "/" => "*",
        ">=" => ">",
        ">" => ">=",
        "<=" => "<",
        "<" => "<=",
        "=" => "!=",
        "!=" => "=",
        _ => return None,
    })
}

fn expr_mutants(e: &Expr, params: &IndexMap<String, Vec<String>>) -> Vec<(String, Expr)> {
    let mut out = vec![];
    if let Expr::Bin(op, l, r) = e {
        if let Some(s) = swap(op) {
            out.push((format!("`{op}` → `{s}`"), Expr::Bin(s, l.clone(), r.clone())));
        }
        for (desc, m) in expr_mutants(l, params) {
            out.push((desc, Expr::Bin(op, Box::new(m), r.clone())));
        }
        for (desc, m) in expr_mutants(r, params) {
            out.push((desc, Expr::Bin(op, l.clone(), Box::new(m))));
        }
    }
    if let Expr::App(name, primed, args) = e {
        if name == "ledger.debit" || name == "ledger.credit" {
            let other = if name == "ledger.debit" { "ledger.credit" } else { "ledger.debit" };
            out.push((format!("`{name}` → `{other}`"), Expr::App(other.to_string(), *primed, args.clone())));
        }
        for (i, a) in args.iter().enumerate() {
            for (desc, m) in expr_mutants(a, params) {
                let mut xs = args.clone();
                xs[i] = m;
                out.push((desc, Expr::App(name.clone(), *primed, xs)));
            }
        }
    }
    if let Expr::Name(n, primed) = e {
        if let Some(ps) = params.get(n) {
            for p in ps {
                out.push((format!("`{n}` → `{p}`"), Expr::Name(p.clone(), *primed)));
            }
        }
    }
    if let Expr::Int(n) = e {
        out.push((format!("`{}` → `{}`", n, n + 1), Expr::Int(n + 1)));
    }
    if let Expr::Name(n, _) = e {
        if n == "clock.now" {
            out.push(("`clock.now` → `0`".into(), Expr::Int(0)));
        }
    }
    out
}

/// Every well-formed-or-not body mutant of d, with its description, in Python's order.
pub fn mutants(d: &Def) -> Vec<(String, Vec<Stmt>)> {
    let mut by_type: IndexMap<String, Vec<String>> = IndexMap::new();
    for (p, t) in &d.params {
        by_type.entry(t.clone()).or_default().push(p.clone());
    }
    let same_type: IndexMap<String, Vec<String>> =
        d.params.iter().map(|(p, t)| (p.clone(), by_type[t].iter().filter(|q| *q != p).cloned().collect())).collect();
    block_mutants(&d.body, &same_type)
}

fn block_mutants(body: &[Stmt], same_type: &IndexMap<String, Vec<String>>) -> Vec<(String, Vec<Stmt>)> {
    let mut out = vec![];
    for (i, s) in body.iter().enumerate() {
        let put = |new: Stmt| -> Vec<Stmt> {
            let mut b = body.to_vec();
            b[i] = new;
            b
        };
        let mut dropped = body.to_vec();
        dropped.remove(i);
        out.push((format!("drop `{}`", stmt_code(s)), dropped));
        match s {
            Stmt::For(var, lo, hi, invs, inner) => {
                let one = Expr::Int(1);
                let sc = stmt_code(s);
                for (desc, m) in [
                    (format!("`{}` → `{} + 1`", code(lo), code(lo)), Expr::Bin("+", Box::new(lo.clone()), Box::new(one.clone()))),
                    (format!("`{}` → `{} - 1`", code(lo), code(lo)), Expr::Bin("-", Box::new(lo.clone()), Box::new(one.clone()))),
                ] {
                    out.push((format!("{desc} in `{sc}`"), put(Stmt::For(var.clone(), m, hi.clone(), invs.clone(), inner.clone()))));
                }
                for (desc, m) in [
                    (format!("`{}` → `{} + 1`", code(hi), code(hi)), Expr::Bin("+", Box::new(hi.clone()), Box::new(one.clone()))),
                    (format!("`{}` → `{} - 1`", code(hi), code(hi)), Expr::Bin("-", Box::new(hi.clone()), Box::new(one.clone()))),
                ] {
                    out.push((format!("{desc} in `{sc}`"), put(Stmt::For(var.clone(), lo.clone(), m, invs.clone(), inner.clone()))));
                }
                let shifted = shift_var(inner, var);
                if &shifted != inner {
                    out.push((
                        format!("`{var}` → `{var} + 1` in the body of `{sc}`"),
                        put(Stmt::For(var.clone(), lo.clone(), hi.clone(), invs.clone(), shifted)),
                    ));
                }
                out.push((format!("empty the body of `{sc}`"), put(Stmt::For(var.clone(), lo.clone(), hi.clone(), invs.clone(), vec![]))));
                for (desc, m) in block_mutants(inner, same_type) {
                    out.push((desc, put(Stmt::For(var.clone(), lo.clone(), hi.clone(), invs.clone(), m))));
                }
            }
            Stmt::If(c, then, els) => {
                let sc = stmt_code(s);
                for (desc, m) in expr_mutants(c, same_type) {
                    out.push((format!("{desc} in `{sc}`"), put(Stmt::If(m, then.clone(), els.clone()))));
                }
                out.push((format!("swap the branches of `{sc}`"), put(Stmt::If(c.clone(), els.clone(), then.clone()))));
                for (desc, m) in block_mutants(then, same_type) {
                    out.push((desc, put(Stmt::If(c.clone(), m, els.clone()))));
                }
                for (desc, m) in block_mutants(els, same_type) {
                    out.push((desc, put(Stmt::If(c.clone(), then.clone(), m))));
                }
            }
            Stmt::Let(x, e) => {
                for (desc, m) in expr_mutants(e, same_type) {
                    out.push((format!("{desc} in `{}`", stmt_code(s)), put(Stmt::Let(x.clone(), m))));
                }
            }
            Stmt::Do(e) => {
                for (desc, m) in expr_mutants(e, same_type) {
                    out.push((format!("{desc} in `{}`", stmt_code(s)), put(Stmt::Do(m))));
                }
            }
            Stmt::Need(c, ctor, args) => {
                for (desc, m) in expr_mutants(c, same_type) {
                    out.push((format!("{desc} in `{}`", stmt_code(s)), put(Stmt::Need(m, ctor.clone(), args.clone()))));
                }
                for (k, a) in args.iter().enumerate() {
                    for (desc, m) in expr_mutants(a, same_type) {
                        let mut xs = args.clone();
                        xs[k] = m;
                        out.push((format!("{desc} in payload of `{}`", stmt_code(s)), put(Stmt::Need(c.clone(), ctor.clone(), xs))));
                    }
                }
            }
            Stmt::Inv(_) => panic!("mutants: Python has no mutants for an `inv` statement in a body (KeyError)"),
        }
    }
    out
}

/// Every use of the loop variable in a body as `var + 1`.
fn shift_var(stmts: &[Stmt], var: &str) -> Vec<Stmt> {
    fn sub(e: &Expr, var: &str) -> Expr {
        match e {
            Expr::Name(n, false) if n == var => Expr::Bin("+", Box::new(e.clone()), Box::new(Expr::Int(1))),
            Expr::Bin(op, l, r) => Expr::Bin(op, Box::new(sub(l, var)), Box::new(sub(r, var))),
            Expr::App(h, p, a) => Expr::App(h.clone(), *p, a.iter().map(|x| sub(x, var)).collect()),
            other => other.clone(),
        }
    }
    stmts
        .iter()
        .map(|s| match s {
            Stmt::Let(x, e) => Stmt::Let(x.clone(), sub(e, var)),
            Stmt::Need(c, ctor, a) => Stmt::Need(sub(c, var), ctor.clone(), a.iter().map(|x| sub(x, var)).collect()),
            Stmt::If(c, t, o) => Stmt::If(sub(c, var), shift_var(t, var), shift_var(o, var)),
            Stmt::Do(e) => Stmt::Do(sub(e, var)),
            other => other.clone(),
        })
        .collect()
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
}

/// Every layer on every well-formed mutant, so the score can say *what* catches bugs.
pub fn mutation_score(defs: &Module, d: &Def, n_props: u64) -> Mutation {
    let (mut rows, mut ill_formed, mut errors) = (vec![], vec![], vec![]);
    for (desc, body) in mutants(d) {
        let mut md = d.clone();
        md.body = body;
        let mdefs = defs.with_def(md.clone());
        if !problems(&mdefs, &md).is_empty() {
            ill_formed.push(desc);
            continue;
        }
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(bool, bool, bool), Crash> {
            let ex = !run_examples(&mdefs, &md)?.iter().all(|r| r.pass);
            let props = !proptest(&mdefs, &md, n_props, 7)?.violations.is_empty();
            let (gs, _) = Prover::new(&mdefs).prove(&md, true);
            let prover = gs.iter().any(|g| matches!(g.status, Status::Refuted | Status::Vacuous));
            Ok((ex, props, prover))
        }));
        match run {
            Ok(Ok((ex, pr, pv))) => rows.push((desc, ex, pr, pv)),
            Ok(Err(Crash(t, m))) => errors.push(format!("{desc}: {t}: {m}")),
            Err(p) => errors.push(format!("{desc}: Panic: {}", panic_text(p))),
        }
    }
    let count = |f: &dyn Fn(&(String, bool, bool, bool)) -> bool| rows.iter().filter(|r| f(r)).count() as u64;
    Mutation {
        total: rows.len() as u64,
        guarantee_killed: count(&|r| r.2 || r.3),
        prover_killed: count(&|r| r.3),
        props_killed: count(&|r| r.2),
        example_killed: count(&|r| r.1),
        killed: count(&|r| r.1 || r.2 || r.3),
        guarantee_missed: rows.iter().filter(|r| !(r.2 || r.3)).map(|r| (r.0.clone(), r.1)).collect(),
        survived: rows.iter().filter(|r| !(r.1 || r.2 || r.3)).map(|r| r.0.clone()).collect(),
        ill_formed,
        errors,
    }
}
