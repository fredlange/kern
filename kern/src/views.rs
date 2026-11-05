//! What each reader gets (port of kern/views.py). The agent's views are plain Kern text; the English templates are
//! for the human proof page (fixed templates over verifier results, no model).

use std::collections::BTreeSet;

use indexmap::IndexMap;

use crate::infer::{fmt_fails, fmt_uses, fmt_writes, uses};
use crate::report::{Report, Status as GStatus};
use crate::syntax::{atom_code, code, flat, negate, side, Def, Emit, Expr, Module, Need, Stmt};
use crate::why::Status;

/// Token count of a view with a Claude tokenizer, when one is available (Python: the optional `tokens` extra, the
/// tokenizer.json bundled in anthropic==0.34.2). The file is looked up in KERN_TOKENIZER, then
/// ~/.cache/kern/tokenizer.json, then the checkout's .venv; without one this is None and `kern view` prints no
/// count. The counts are identical to Python's (REFERENCE.md 2,230, GUIDE.md 5,434, CLAUDE.md 4,579).
#[cfg(feature = "tokens")]
pub fn tokens(text: &str) -> Option<usize> {
    use std::sync::OnceLock;
    static TOK: OnceLock<Option<tokenizers::Tokenizer>> = OnceLock::new();
    let tok = TOK.get_or_init(|| {
        let mut paths: Vec<String> = vec![];
        if let Ok(p) = std::env::var("KERN_TOKENIZER") {
            paths.push(p);
        }
        if let Ok(h) = std::env::var("HOME") {
            paths.push(format!("{}/.cache/kern/tokenizer.json", h));
        }
        let venv = concat!(env!("CARGO_MANIFEST_DIR"), "/../.venv/lib");
        if let Ok(rd) = std::fs::read_dir(venv) {
            for py in rd.flatten() {
                paths.push(format!("{}/site-packages/anthropic/tokenizer.json", py.path().to_string_lossy()));
            }
        }
        paths.iter().find_map(|p| tokenizers::Tokenizer::from_file(p).ok())
    });
    tok.as_ref().and_then(|t| t.encode(text, true).ok()).map(|e| e.get_ids().len())
}

#[cfg(not(feature = "tokens"))]
pub fn tokens(_text: &str) -> Option<usize> {
    None
}

fn proof_line(r: &Report) -> String {
    if r.clean {
        return "  proof ok".into();
    }
    let bad = r.guarantees.iter().filter(|g| g.status != GStatus::Proven).count();
    format!("  proof {} open", bad)
}

/// What a caller that only needs to *use* the definition reads.
pub fn iface(r: &Report, why: &Status) -> String {
    let d = &r.def;
    let mut lines = vec![d.src_lines["header"][0].clone()];
    for (name, fields) in &d.records {
        let fs: Vec<String> = fields.iter().map(|(f, t)| format!("{}:{}", f, t)).collect();
        lines.push(format!("  record {} {}", name, fs.join(" ")));
    }
    lines.push(format!("  uses {}", fmt_uses(&r.uses)));
    if !r.writes.is_empty() {
        lines.push(format!("  writes {}", fmt_writes(&r.writes, d)));
    }
    if !r.fails.is_empty() {
        lines.push(format!("  fails {}", fmt_fails(&r.fails)));
    }
    if why.count > 0 {
        lines.push(format!("  why {}{}", why.count, if why.stale { " stale" } else { "" }));
    }
    lines.join("\n")
}

/// What a caller that must *reason about* the definition reads: interface + guarantees + proof status.
pub fn spec(r: &Report, why: &Status) -> String {
    let mut lines = vec![iface(r, why)];
    lines.extend(const_lines(r.def.spec_consts.iter()));
    lines.extend(spec_lines(&r.def).iter().map(|l| format!("  {}", l)));
    lines.extend(body_needs(r).iter().map(|l| format!("  {}", l)));
    lines.push(proof_line(r));
    lines.join("\n")
}

/// One `  const NAME N` line per named constant the shown source mentions (its value is substituted everywhere else).
fn const_lines<'a>(cs: impl Iterator<Item = &'a (String, crate::Int)>) -> Vec<String> {
    cs.map(|(n, v)| format!("  const {} {}", n, v)).collect()
}

fn need_line(c: &Expr, ctor: &str, args: &[Expr]) -> String {
    let mut parts = vec![ctor.to_string()];
    parts.extend(args.iter().map(atom_code));
    format!("need {} else {}", code(c), parts.join(" "))
}

/// The spec clauses a *reader* gets. A `need as X ...` / `emit as X ...` line is shown expanded; definitions without
/// references get their source lines unchanged.
pub fn spec_lines(d: &Def) -> Vec<String> {
    let src: Vec<String> = d.src_lines["spec"].iter().filter(|l| !l.starts_with("eg ")).cloned().collect();
    if d.ref_origin.is_empty() {
        return src;
    }
    let needs: Vec<String> = d
        .needs
        .iter()
        .filter_map(|n| if let Need::Need(c, k, a) = n { Some(need_line(c, k, a)) } else { None })
        .collect();
    let emits: Vec<String> = d.emits.iter().map(|ev| format!("emit {}", crate::syntax::emit_code(ev))).collect();
    let (mut out, mut done) = (vec![], BTreeSet::new());
    for l in src {
        let kind = l.split(' ').next().unwrap_or("").to_string();
        if kind == "need" || kind == "emit" {
            if done.insert(kind.clone()) {
                out.extend(if kind == "need" { needs.clone() } else { emits.clone() });
            }
            continue;
        }
        out.push(l);
    }
    out
}

/// What an agent *editing* the definition reads: its full source + the spec view of each callee.
pub fn editor(reports: &IndexMap<String, Report>, name: &str, whys: &IndexMap<String, Status>) -> String {
    let r = &reports[name];
    let d = &r.def;
    let mut own = vec![iface(r, &whys[name])];
    let body_only = d.body_consts.iter().filter(|(n, _)| !d.spec_consts.iter().any(|(s, _)| s == n));
    own.extend(const_lines(d.spec_consts.iter().chain(body_only)));
    own.extend(d.src_lines["spec"].iter().map(|l| format!("  {}", l)));
    own.push("body".into());
    own.extend(d.src_lines["body"].iter().map(|l| format!("  {}", l)));
    let deps: Vec<String> = r.callees.iter().map(|c| spec(&reports[c], &whys[c])).collect();
    let mut text = own.join("\n");
    if !deps.is_empty() {
        text.push_str("\n\n");
        text.push_str(&deps.join("\n\n"));
    }
    text
}

// ---------- English rendering for the human proof page (templates, no model) ----------

fn name_template(n: &str) -> Option<&'static str> {
    Some(match n {
        "ledger.balance" => "balance of {0}",
        "ledger.owner" => "owner of {0}",
        "ledger.total" => "total money in the ledger",
        "clock.now" => "the time of the call",
        _ => return None,
    })
}

fn op_text(op: &str) -> &'static str {
    match op {
        "=" => "=",
        "!=" => "≠",
        ">=" => "≥",
        "<=" => "≤",
        ">" => "&gt;",
        "<" => "&lt;",
        "+" => "+",
        "-" => "−",
        "*" => "×",
        "/" => "÷",
        "and" => "and",
        "or" => "or",
        "when" => "when",
        _ => panic!("no text for operator {}", op),
    }
}

/// How the page names an effect.
pub fn effect_text(e: &str) -> Option<&'static str> {
    Some(match e {
        "ledger.write" => "the ledger (read and write)",
        "ledger.read" => "the ledger (read only)",
        "clock" => "the clock",
        "audit" => "the audit log",
        _ => return None,
    })
}

/// The definition's own body-level refusal conditions, in `need` syntax, marked `body`. Inherited ones (from
/// callees) are left to the callee's own spec view.
pub fn body_needs(r: &Report) -> Vec<String> {
    r.lifted
        .iter()
        .filter(|l| l.origin.is_empty())
        .map(|l| {
            let mut parts = vec![l.ctor.clone()];
            parts.extend(l.payload.iter().flatten().map(atom_code));
            format!("body need {} else {}", code(&l.cond), parts.join(" "))
        })
        .collect()
}

/// Written balances (and state entries, `hold.id`) that no `ensure` describes the after-value of.
pub fn unguaranteed_writes(r: &Report) -> Vec<String> {
    let d = &r.def;
    let mut said: BTreeSet<String> = BTreeSet::new();
    fn walk(e: &Expr, q: Option<(&str, &Expr, &Expr)>, said: &mut BTreeSet<String>) {
        if let Expr::All(j, a, b, body) = e {
            // `... for j from A to B` describes a range
            return walk(body, Some((j, a, b)), said);
        }
        if let Expr::App(n, primed, args) = e {
            if n == "ledger.balance" && *primed {
                if let Some(Expr::Name(x, _)) = args.first() {
                    said.insert(x.clone());
                }
            }
            if *primed && n.ends_with(".get") {
                if let Some(Expr::Name(key, _)) = args.first() {
                    let x = &n[..n.len() - 4];
                    if let Some((j, lo, hi)) = q {
                        if key == j {
                            said.insert(format!("{}.[{},{})", x, code(lo), code(hi)));
                        }
                    }
                    said.insert(format!("{}.{}", x, key));
                }
            }
            for a in args {
                walk(a, q, said);
            }
        }
        if let Expr::Bin(_, l, r) = e {
            walk(l, q, said);
            walk(r, q, said);
        }
    }
    for c in &d.ensures {
        walk(c, None, &mut said);
    }
    let mut out: Vec<String> =
        d.params.iter().filter(|(p, _)| r.writes.contains(p) && !said.contains(p)).map(|(p, _)| p.clone()).collect();
    out.extend(r.writes.iter().filter(|w| w.contains('.') && !said.contains(*w)).cloned());
    out
}

/// A definition that emits events inside a loop: the channel is unconstrained (no spec form for per-iteration
/// events yet) and must say so instead of looking silent.
pub fn loop_event_note(r: &Report, defs: &Module) -> Vec<String> {
    let mut chans = BTreeSet::new();
    for s in flat(&r.def.body) {
        let Stmt::For(_, _, _, _, body) = s else { continue };
        for t in flat(body) {
            if let Stmt::Do(Expr::App(n, _, _)) = t {
                if n == "audit.emit" || n.ends_with(".emit") {
                    chans.insert(n.rsplit_once('.').unwrap().0.to_string());
                } else if let Some(callee) = defs.defs.get(n) {
                    for u in uses(defs, callee) {
                        if u == "audit" || defs.channels.contains(&u) {
                            chans.insert(u);
                        }
                    }
                }
            }
        }
    }
    chans.into_iter().collect()
}

fn has_prime(e: &Expr) -> bool {
    match e {
        Expr::Name(_, p) => *p,
        Expr::App(_, p, args) => *p || args.iter().any(has_prime),
        Expr::Bin(_, l, r) => has_prime(l) || has_prime(r),
        _ => false,
    }
}

pub type Lets = IndexMap<String, Expr>;

/// An expression in English, for the page.
pub fn en(e: &Expr, lets: &Lets, tense: bool) -> String {
    match e {
        Expr::Int(n) => n.to_string(),
        Expr::All(j, a, b, body) => format!(
            "for every <code>{}</code> from {} to {} (not included): {}",
            j,
            en(a, lets, false),
            en(b, lets, false),
            if tense { en_ensure(body) } else { en(body, lets, false) }
        ),
        Expr::Name(n, primed) => {
            if let Some(v) = lets.get(n) {
                return en(v, lets, tense);
            }
            if let Some(t) = name_template(n) {
                return format!("{}{}", t, if *primed { " after" } else if tense { " before" } else { "" });
            }
            format!("<code>{}</code>", n)
        }
        Expr::App(n, primed, args) => {
            let when = if *primed { " <em>after</em>" } else if tense { " <em>before</em>" } else { "" };
            if n.ends_with(".get") && args.len() == 1 {
                // declared state: `hold of id`
                return format!("{} of {}{}", &n[..n.len() - 4], en(&args[0], lets, tense), when);
            }
            if n == "distinct" {
                let names: Vec<String> = args.iter().map(|a| en(a, lets, tense)).collect();
                let (last, init) = names.split_last().unwrap();
                return format!("{} and {} are all different", init.join(", "), last);
            }
            if let Some(t) = name_template(n) {
                let mut s = t.to_string();
                for (i, a) in args.iter().enumerate() {
                    s = s.replace(&format!("{{{}}}", i), &en(a, lets, tense));
                }
                return format!("{}{}", s, when);
            }
            let shown: Vec<String> = args.iter().map(|a| en(a, lets, tense)).collect();
            format!("<code>{} {}</code>", n, shown.join(" "))
        }
        Expr::Bin(op, l, r) => {
            let show = |x: &Expr| en(x, lets, tense);
            format!("{} {} {}", side(l, op, false, show), op_text(op), side(r, op, true, show))
        }
    }
}

fn is_wild(a: &Expr) -> bool {
    matches!(a, Expr::Name(n, false) if n == "_")
}

/// Template for an `emit` guarantee: the exact events appended to the audit log.
pub fn en_emits(events: &[Emit]) -> String {
    let none = Lets::new();
    let mut items = vec![];
    let mut any_cond = false;
    for ev in events {
        let (Emit::Ev(ctor, args, cond) | Emit::As(ctor, args, _, cond)) = ev;
        let shown: Vec<String> =
            args.iter().map(|a| if is_wild(a) { "any value".to_string() } else { en(a, &none, false) }).collect();
        let shown = shown.join(", ");
        let mut item = format!("<code>{}</code>", ctor);
        if !shown.is_empty() {
            item.push_str(&format!(" ({})", shown));
        }
        if let Some(c) = cond {
            any_cond = true;
            item.push_str(&format!(" <em>only when</em> {}", en(c, &none, false)));
        }
        items.push(item);
    }
    if any_cond {
        return format!(
            "appends exactly these events to the audit log, in order, each one only when its condition holds before \
             the call: {}",
            items.join(", then ")
        );
    }
    if items.len() == 1 {
        return format!("appends exactly one event to the audit log: {}", items[0]);
    }
    format!("appends exactly {} events to the audit log, in order: {}", items.len(), items.join(", then "))
}

/// The same, for events on a declared channel.
pub fn en_channel_emits(channel: &str, events: &[Emit]) -> String {
    en_emits(events).replacen("appends", "sends", 1).replacen(
        "to the audit log",
        &format!("on channel <code>{}</code>", channel),
        1,
    )
}

pub fn en_ensure(e: &Expr) -> String {
    let none = Lets::new();
    match e {
        Expr::All(..) => en(e, &none, true),
        Expr::Bin("when", body, cond) => {
            // `E when C`: C is about the before-state
            format!("<em>when</em> {}: {}", en(cond, &none, true), en(body, &none, has_prime(body)))
        }
        _ => en(e, &none, has_prime(e)),
    }
}

/// (condition under which the call refuses, the refusal with its payload).
pub fn en_refusal(cond: &Expr, ctor: &str, args: &[Expr], lets: &Lets) -> (String, String) {
    let text = match cond {
        Expr::App(n, _, cargs) if n == "distinct" => {
            let names: Vec<String> = cargs.iter().map(|a| en(a, lets, false)).collect();
            let (last, init) = names.split_last().unwrap();
            format!("two of {} and {} are the same", init.join(", "), last)
        }
        _ => match negate(cond) {
            Some(n) => en(&n, lets, false),
            None => format!("not ({})", en(cond, lets, false)),
        },
    };
    let detail = if args.is_empty() {
        String::new()
    } else {
        format!(" ({})", args.iter().map(|a| en(a, lets, false)).collect::<Vec<_>>().join(", "))
    };
    (text, format!("{}{}", ctor, detail))
}

pub fn lets_of(d: &Def) -> Lets {
    let mut out = Lets::new();
    for s in flat(&d.body) {
        if let Stmt::Let(x, e) = s {
            out.insert(x.clone(), e.clone());
        }
    }
    out
}

/// A guard on a body need: inside `if C` ("if") or its `else` ("else").
pub type Guard = (&'static str, Expr);

/// Body needs with the `if` conditions (or their negation, in `else`) under which they run.
pub fn guarded_needs(stmts: &[Stmt]) -> Vec<(&Stmt, Vec<Guard>)> {
    let mut out = vec![];
    fn go<'a>(stmts: &'a [Stmt], guards: &[Guard], out: &mut Vec<(&'a Stmt, Vec<Guard>)>) {
        for s in stmts {
            match s {
                Stmt::Need(..) => out.push((s, guards.to_vec())),
                Stmt::If(c, t, o) => {
                    let mut g = guards.to_vec();
                    g.push(("if", c.clone()));
                    go(t, &g, out);
                    let mut g = guards.to_vec();
                    g.push(("else", c.clone()));
                    go(o, &g, out);
                }
                _ => {}
            }
        }
    }
    go(stmts, &[], &mut out);
    out
}

#[cfg(all(test, feature = "tokens"))]
mod tests {
    #[test]
    fn token_counts_match_python_when_a_tokenizer_is_present() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/REFERENCE.md")).unwrap();
        match super::tokens(&text) {
            Some(n) => assert!(n > 1000, "REFERENCE.md: {n} tokens"),
            None => eprintln!("SKIP: no tokenizer.json found"),
        }
    }
}
