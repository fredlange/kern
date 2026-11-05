//! Proof page (HTML) and terminal summary (port of kern/render.py), rendered from verifier results with fixed
//! templates. Nothing here is written by a language model, so the page cannot say more than the checks established.

use indexmap::IndexMap;

use crate::infer::{fails as inferred_fails, fmt_writes};
use crate::package::relpath;
use crate::report::{GoalExpr, Guarantee, LoopAst, Report, Status as G, Violation};
use crate::run::emit_channels;
use crate::syntax::{code, flat, negate, Module, Need, Stmt};
use crate::views::{
    effect_text, en, en_channel_emits, en_emits, en_ensure, en_refusal, guarded_needs, lets_of, loop_event_note,
    unguaranteed_writes, Lets,
};
use crate::why::Status;

/// Python's html.escape (quote=True).
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

pub fn icon(s: G) -> &'static str {
    match s {
        G::Proven => "✔",
        G::Refuted => "✗",
        G::Conditional => "!",
        G::Unknown | G::Unconfirmed => "?",
        G::Tested => "◐",
        G::Vacuous => "∅",
        G::Assumed => "⊙",
    }
}

/// Python's round() of a/b to an integer: halves go to the even neighbour.
fn round_half_even(num: u64, den: u64) -> u64 {
    let (q, r) = (num / den, num % den);
    match (2 * r).cmp(&den) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => q + (q % 2),
    }
}

/// (badge class, label) for a definition.
pub fn status(r: &Report) -> (&'static str, String) {
    let gs = &r.guarantees;
    let any = |s: G| gs.iter().any(|g| g.status == s);
    if any(G::Vacuous) {
        return ("fail", "Never succeeds".into());
    }
    if any(G::Refuted) || !r.props.violations.is_empty() || !r.examples.iter().all(|e| e.pass) {
        return ("fail", "Broken guarantees".into());
    }
    if !r.regression.is_empty() {
        if r.regression.iter().all(|x| x.starts_with("spec changed")) {
            return ("warn", "Spec changed since baseline".into());
        }
        return ("warn", "Weaker than baseline".into());
    }
    if gs.is_empty() {
        return ("warn", "No guarantees stated".into());
    }
    if any(G::Conditional) {
        return ("warn", "Depends on unproven code".into());
    }
    if any(G::Unconfirmed) {
        return ("warn", "Not proven: counterexample not reproduced".into());
    }
    if any(G::Tested) {
        return ("warn", "Tested only, not proven".into());
    }
    if gs.iter().any(|g| g.status != G::Proven && g.status != G::Assumed) {
        return ("warn", "Not proven: prover gave no answer".into());
    }
    let mut assumed: Vec<&String> = gs.iter().flat_map(|g| g.assuming.iter().flatten()).collect();
    assumed.sort();
    assumed.dedup();
    if !assumed.is_empty() {
        return ("ok", format!("Proven, assuming {}", assumed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    ("ok", "All guarantees proven".into())
}

fn guarantee_text(g: &Guarantee) -> String {
    let none = Lets::new();
    match (g.kind.as_str(), &g.expr) {
        ("emit", GoalExpr::Emits(evs)) => en_emits(evs),
        ("emit", GoalExpr::ChannelEmits(ch, evs)) => en_channel_emits(ch, evs),
        ("ok", _) => "succeeds whenever the needs above hold: it refuses for no other reason".into(),
        ("loop", GoalExpr::Loop(part, ast)) => {
            let when = if part == "entry" { "holds when the loop starts" } else { "is kept by every iteration" };
            let what = match ast {
                LoopAst::Module(text) => escape(text),
                LoopAst::Inv(e) => en_ensure(e),
            };
            format!(
                "loop invariant ({}; at the start of each iteration, primed reads are the current state and unprimed \
                 reads the state before the call): {}",
                when, what
            )
        }
        ("invariant", GoalExpr::Invariant(var, t, cond)) => {
            format!("keeps the module invariant for every <code>{}</code> ({}): {}", var, t, en(cond, &none, false))
        }
        ("keep", GoalExpr::Expr(e)) => format!("{} is unchanged", en(e, &none, false)),
        (_, GoalExpr::Expr(e)) => en_ensure(e),
        // Python renders the (handler, need) pair of a call-site goal through en_ensure, which gives None
        _ => "None".into(),
    }
}

fn how(g: &Guarantee) -> String {
    match g.status {
        G::Proven => "Proven for all inputs (Z3).".into(),
        G::Assumed => {
            let who: Vec<String> =
                g.assuming.iter().flatten().map(|a| format!("<code>{}</code>", escape(a))).collect();
            format!(
                "Proven for all inputs, <em>assuming</em> the declared spec of {}: an observation the host \
                 provides. The host checks that spec on every call; the prover can't.",
                who.join(", ")
            )
        }
        G::Conditional => {
            let who: Vec<String> = g.blocked_by.iter().flatten().map(|c| format!("<code>{}</code>", c)).collect();
            if g.via_broken_spec == Some(true) {
                return format!(
                    "Unknown: it fails according to the spec of {}, but that spec is itself broken, and the \
                     interpreter did not reproduce the failure.",
                    who.join(", ")
                );
            }
            format!("Proven only if the guarantees of {} hold — and they are not proven.", who.join(", "))
        }
        G::Vacuous => format!("Holds only vacuously: {}.", escape(g.reason.as_deref().unwrap_or(""))),
        G::Tested => format!(
            "Not proven. Held in all {} random runs that succeeded{}",
            g.tested.unwrap_or(0),
            match g.reason.as_deref() {
                Some(r) if !r.is_empty() => format!(" ({}).", escape(r)),
                _ => ".".into(),
            }
        ),
        G::Refuted | G::Unconfirmed => {
            let cx = g.counterexample.as_ref().expect("a refuted goal has a counterexample");
            let conf = if cx.confirmed_by_interpreter {
                " Replayed in the interpreter: confirmed."
            } else {
                " Not reproduced in the interpreter, so this is not evidence of a bug: a callee's spec may be too \
                 weak to prove the guarantee."
            };
            let got = if g.kind == "emit" {
                format!(" The audit log received: {}.", escape(&cx.emitted))
            } else {
                String::new()
            };
            format!("Fails for this input:<pre>{}</pre>{}{}", escape(&cx.eg), got, conf)
        }
        G::Unknown => match g.reason.as_deref() {
            Some(r) if !r.is_empty() => format!("Cannot be proven: {}.", escape(r)),
            _ => "Prover gave no answer within the time limit.".into(),
        },
    }
}

fn effect_list(r: &Report) -> String {
    let parts: Vec<String> = r
        .uses
        .iter()
        .map(|e| match effect_text(e) {
            Some(t) => t.to_string(),
            None => format!(
                "state <code>{}</code> (read {})",
                e.rsplit_once('.').map(|x| x.0).unwrap_or(e),
                if e.ends_with("write") { "and write" } else { "only" }
            ),
        })
        .collect();
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(", ")
    }
}

fn all_dotted(xs: &[String]) -> bool {
    xs.iter().all(|x| x.contains('.'))
}

pub fn card(r: &Report, defs: &Module, why: &Status) -> String {
    let d = &r.def;
    let (cls, label) = status(r);
    let lets = lets_of(d);
    let module = d.module.as_ref().map(|m| format!("<span class=\"muted\"> in {}</span>", escape(m))).unwrap_or_default();
    let mut out = vec![format!(
        "<section class=\"card\"><header><h2><code>{}</code>{}</h2><span class=\"badge {}\">{}</span><span \
         class=\"hash\">build {}</span></header>",
        d.name, module, cls, label, r.hash
    )];
    out.push(format!(
        "<h3>Can touch</h3><p>{}. <span class=\"muted\">Inferred from the body; nothing else is reachable.</span></p>",
        effect_list(r)
    ));
    if !r.writes.is_empty() {
        let what = if r.writes.iter().any(|w| w.contains('.')) { "Changes" } else { "Changes balances of" };
        out.push(format!(
            "<p>{}: {}. <span class='muted'>Inferred; every other {} is unchanged.</span></p>",
            what,
            escape(&fmt_writes(&r.writes, d)),
            if what == "Changes" { "balance and state entry" } else { "balance" }
        ));
        let silent = unguaranteed_writes(r);
        if !silent.is_empty() {
            out.push(format!(
                "<p class='warnc'>No guarantee says what the new {} of {} is.</p>",
                if all_dotted(&silent) { "value" } else { "balance" },
                silent.iter().map(|p| format!("<code>{}</code>", p)).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    for ch in loop_event_note(r, defs) {
        out.push(format!(
            "<p class='warnc'>Events on <code>{}</code> emitted inside a loop are not described by any spec: a \
             caller can't rely on them (there is no per-iteration emit form yet).</p>",
            escape(&ch)
        ));
    }

    let mut rows: Vec<(String, String)> = vec![];
    // (cond, ctor, payload, guards, `need as` origin, after): `after` marks a body need that reads state the call
    // already changed. Its source text is then about the state after those changes, which the reader can't see:
    // the row shows the lifted pre-state form instead (its guards folded in), or says so when there is none (S18).
    type NeedRow<'a> = (crate::syntax::Expr, &'a str, Vec<crate::syntax::Expr>, Vec<crate::views::Guard>, Option<&'a String>, Option<bool>);
    let mut needs: Vec<NeedRow> = vec![];
    for (i, n) in d.needs.iter().enumerate() {
        if let Need::Need(c, k, a) = n {
            needs.push((c.clone(), k, a.clone(), vec![], d.ref_origin.get(&("need".to_string(), i)), None));
        }
    }
    use crate::infer::AfterWrites;
    let post = crate::infer::needs_after_writes(defs, d);
    for (i, (s, guards)) in guarded_needs(&d.body).into_iter().enumerate() {
        if let Stmt::Need(c, k, a) = s {
            if let Some(AfterWrites::Yes { one_case }) = post.get(i).copied() {
                // a pre-state form with an aliasing split (`to = from and … or …`) reads worse than the source
                // text with a note, so only a one-case form replaces it
                let l = r.lifted.iter().find(|l| l.origin.is_empty() && l.at == Some(i)).filter(|_| one_case);
                let payload = l.and_then(|l| l.payload.clone().or_else(|| a.is_empty().then(Vec::new)));
                if let (Some(l), Some(p)) = (l, payload) {
                    needs.push((simplify(&l.cond), &l.ctor, p.iter().map(simplify).collect(), vec![], None, Some(true)));
                    continue;
                }
                needs.push((c.clone(), k, a.clone(), guards, None, Some(false)));
                continue;
            }
            needs.push((c.clone(), k, a.clone(), guards, None, None));
        }
    }
    let no_lets = Lets::new();
    for (c, ctor, args, guards, origin, after) in &needs {
        let lets = if *after == Some(true) { &no_lets } else { &lets };
        let (mut cond, _) = en_refusal(c, ctor, args, lets);
        if *after == Some(false) {
            // not lifted: the condition is checked on the state after the call's earlier changes
            cond.push_str(" <span class='muted'>(after this call's earlier changes)</span>");
        }
        if let Some(o) = origin {
            // written as `need as X ...`: X's spec, these arguments
            cond.push_str(&format!(" <span class='muted'>(as {})</span>", escape(o)));
        }
        for (kind, g) in guards.iter().rev() {
            // a need inside `if G` only applies when G holds
            let g_text = if *kind == "if" {
                en(g, &lets, false)
            } else {
                match negate(g) {
                    Some(n) => en(&n, &lets, false),
                    None => format!("not ({})", en(g, &lets, false)),
                }
            };
            cond = format!("{} <span class='muted'>and</span> {}", g_text, cond);
        }
        let detail = if args.is_empty() {
            String::new()
        } else {
            format!(
                " <span class='muted'>with</span> {}",
                args.iter().map(|a| en(a, &lets, false)).collect::<Vec<_>>().join(", ")
            )
        };
        rows.push((cond, format!("<code>{}</code>{}", escape(ctor), detail)));
    }
    let ok = r.guarantees.iter().find(|g| g.kind == "ok");
    let complete = ok.map_or(false, |g| g.status == G::Proven || g.status == G::Conditional);
    if !complete {
        for l in &r.lifted {
            if !l.origin.is_empty() {
                // own body needs are already listed above
                let payload = l.payload.clone().unwrap_or_default();
                let (text, _) = en_refusal(&l.cond, &l.ctor, &payload, &Lets::new());
                rows.push((
                    text,
                    format!("<code>{}</code> <span class='muted'>(from {})</span>", escape(&l.ctor), l.origin),
                ));
            }
        }
        let stated: Vec<&String> = r.lifted.iter().map(|l| &l.ctor).collect();
        let mut own: Vec<&String> = vec![];
        for n in &d.needs {
            if let Need::Need(_, k, _) = n {
                own.push(k);
            }
        }
        for s in flat(&d.body) {
            if let Stmt::Need(_, k, _) = s {
                own.push(k);
            }
        }
        for c in &r.callees {
            let inherited: Vec<String> = inferred_fails(defs, &defs.defs[c])
                .into_keys()
                .filter(|k| !own.contains(&k) && !stated.contains(&k))
                .collect();
            if !inherited.is_empty() {
                rows.push((
                    format!("<code>{}</code> refuses", c),
                    format!(
                        "{} <span class='muted'>(passed through)</span>",
                        inherited.iter().map(|k| format!("<code>{}</code>", k)).collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
        }
    }
    if !rows.is_empty() {
        let body: String =
            rows.iter().map(|(c, f)| format!("<tr><td>{}</td><td class='fail-name'>→ {}</td></tr>", c, f)).collect();
        out.push(format!(
            "<h3>Refuses when</h3><table>{}</table><p class='muted'>A refusal undoes everything the call did.{}</p>",
            body,
            if complete { " It refuses for no other reason (see <code>ok otherwise</code> below)." } else { "" }
        ));
    }

    out.push("<h3>Guarantees</h3><ul class='g'>".into());
    if emit_channels(d).len() > 1 {
        out.push(
            "<p class='muted'>Each channel's events are guaranteed in order; the order <em>across</em> channels is \
             not (the host delivers audit first, then channels in declaration order).</p>"
                .into(),
        );
    }
    for g in &r.guarantees {
        out.push(format!(
            "<li class='{}'><span class='ic'>{}</span><div><div>{}</div><div class='how'>{}</div><div \
             class='src'><code>{}</code></div></div></li>",
            g.status.as_str(),
            icon(g.status),
            guarantee_text(g),
            how(g),
            escape(&format!("{} {}", g.kind, g.clause))
        ));
    }
    out.push("</ul>".into());

    let ex = &r.examples;
    let passed = ex.iter().filter(|e| e.pass).count();
    out.push(format!(
        "<h3>Examples <span class='muted'>{}/{} pass, executed against build {}</span></h3><ul class='ex'>",
        passed,
        ex.len(),
        r.hash
    ));
    for e in ex {
        let mark = if e.pass { "✔".to_string() } else { format!("✗ got {}", escape(&e.got)) };
        out.push(format!(
            "<li><code>{}</code> <span class='{}'>{}</span></li>",
            escape(&e.src),
            if e.pass { "okc" } else { "failc" },
            mark
        ));
    }
    out.push("</ul>".into());

    let p = &r.props;
    let mut fl: Vec<(&String, &u64)> = p.failures.iter().collect();
    fl.sort_by(|a, b| b.1.cmp(a.1));
    let fl: Vec<String> = fl.iter().map(|(k, v)| format!("{} {}", k, v)).collect();
    out.push(format!(
        "<h3>Random tests</h3><p>{} random worlds and inputs: {} succeeded and were checked against every guarantee; \
         refused: {}.</p>",
        p.runs,
        p.succeeded,
        if fl.is_empty() { "none".to_string() } else { fl.join(", ") }
    ));
    if !p.violations.is_empty() {
        let lines: Vec<String> = p
            .violations
            .iter()
            .map(|v| match v {
                Violation::Clause { clause, eg } => format!("{}   # breaks: {}", eg, clause),
                Violation::Error { error, .. } => format!("   # breaks: {}", error),
            })
            .collect();
        out.push(format!("<p class='failc'>Guarantee violated:</p><pre>{}</pre>", escape(&lines.join("\n"))));
    }

    match &r.mutation {
        Some(m) => {
            let pct = if m.total > 0 { round_half_even(100 * m.guarantee_killed, m.total) } else { 100 };
            out.push(format!(
                "<h3>Spec strength <span class='muted'>guarantees catch {}/{} deliberate bugs ({}%)</span></h3><p>Caught \
                 by the prover: {}; by random tests checking the guarantees: {}. The examples alone catch {}/{}; \
                 everything together {}/{}.</p>",
                m.guarantee_killed,
                m.total,
                pct,
                m.prover_killed,
                m.props_killed,
                m.example_killed,
                m.total,
                m.killed,
                m.total
            ));
            if !m.guarantee_missed.is_empty() {
                let items: String = m
                    .guarantee_missed
                    .iter()
                    .map(|(s, ex)| {
                        format!(
                            "<li>{}{}</li>",
                            escape(s),
                            if *ex {
                                " <span class='muted'>(an example catches it)</span>"
                            } else {
                                " <span class='failc'>(nothing catches it)</span>"
                            }
                        )
                    })
                    .collect();
                out.push(format!(
                    "<p>No guarantee rules out these changes to the body. Either a guarantee is missing, or the code \
                     is unnecessary:</p><ul class='ex'>{}</ul>",
                    items
                ));
            }
            if !m.errors.is_empty() {
                let items: String = m.errors.iter().map(|e| format!("<li>{}</li>", escape(e))).collect();
                out.push(format!(
                    "<p class='failc'>The checker crashed on these mutants (not counted):</p><ul class='ex'>{}</ul>",
                    items
                ));
            }
            if let Some(sm) = &r.spec_mutation {
                out.push(spec_mutation_html(sm));
            }
        }
        None => out.push(
            "<h3>Spec strength</h3><p class='muted'>Not measured until every guarantee holds.</p>".into(),
        ),
    }

    if !r.regression.is_empty() {
        let items: String = r.regression.iter().map(|x| format!("<li class='failc'>{}</li>", escape(x))).collect();
        out.push(format!(
            "<h3>Changed since the baseline</h3><p class='muted'>What callers could rely on at the baseline and can't \
             now. A change may be intended; it must not be invisible.</p><ul class='ex'>{}</ul>",
            items
        ));
    }

    if !r.relied_on.is_empty() {
        out.push(format!(
            "<h3>Relies on</h3><p>{} — through their specs only, not their code.</p>",
            r.relied_on.iter().map(|c| format!("<code>{}</code>", c)).collect::<Vec<_>>().join(", ")
        ));
    }

    if why.count > 0 {
        let mut stale = if why.stale {
            format!(
                "<p class='failc'>Written for {}; the definition or its inferred interface has changed since (now {}). \
                 Confirm or update.</p>",
                why.anchored_to.as_deref().unwrap_or(""),
                why.current.as_deref().unwrap_or("")
            )
        } else {
            String::new()
        };
        if !why.obligations.is_empty() {
            stale.push_str(&format!(
                "<p class='warnc'>These read like obligations. Anything that must be obeyed belongs in a need, keep, \
                 ensure or emit, not only in prose:</p><ul>{}</ul>",
                why.obligations.iter().map(|e| format!("<li>{}</li>", escape(e))).collect::<String>()
            ));
        }
        out.push(format!(
            "<details><summary>Why ({})</summary>{}<ul>{}</ul></details>",
            why.count,
            stale,
            why.entries.iter().map(|e| format!("<li>{}</li>", escape(e))).collect::<String>()
        ));
    }
    out.push("</section>".into());
    out.join("\n")
}

pub const CSS: &str = r#"
:root{--bg:#f7f7f5;--card:#fff;--fg:#1d1d1b;--muted:#6b6b66;--line:#e4e4df;--ok:#1f7a4d;--fail:#b3261e;--warn:#a15c00;--code:#f0f0ec}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#161615;--card:#1f1f1d;--fg:#ececea;--muted:#9a9a94;--line:#33332f;--ok:#5cc28d;--fail:#ff7b72;--warn:#e3a54a;--code:#2a2a27}}
:root[data-theme="dark"]{--bg:#161615;--card:#1f1f1d;--fg:#ececea;--muted:#9a9a94;--line:#33332f;--ok:#5cc28d;--fail:#ff7b72;--warn:#e3a54a;--code:#2a2a27}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:860px;margin:0 auto;padding:32px 16px}h1{font-size:22px;margin:0 0 4px}.sub{color:var(--muted);margin:0 0 24px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:20px 22px;margin-bottom:20px}
.card header{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.card h2{margin:0;font-size:19px}
.badge{font-size:12px;font-weight:600;padding:2px 9px;border-radius:99px;border:1px solid currentColor}
.badge.ok{color:var(--ok)}.badge.fail{color:var(--fail)}.badge.warn{color:var(--warn)}.hash{margin-left:auto;color:var(--muted);font:12px ui-monospace,monospace}
h3{font-size:13px;text-transform:uppercase;letter-spacing:.04em;color:var(--muted);margin:18px 0 6px}h3 .muted{text-transform:none;letter-spacing:0;font-weight:400}
p{margin:4px 0}.muted{color:var(--muted)}code,pre{font:13px ui-monospace,SFMono-Regular,Menlo,monospace}code{background:var(--code);padding:1px 5px;border-radius:4px}
pre{background:var(--code);padding:8px 10px;border-radius:6px;overflow-x:auto;margin:6px 0}
table{border-collapse:collapse}td{padding:3px 14px 3px 0;vertical-align:top}
ul.g{list-style:none;padding:0;margin:0}ul.g li{display:flex;gap:10px;padding:8px 0;border-top:1px solid var(--line)}ul.g li:first-child{border-top:0}
.ic{font-weight:700;width:16px;flex:none}li.proven .ic{color:var(--ok)}li.refuted .ic{color:var(--fail)}li.conditional .ic,li.unknown .ic{color:var(--warn)}
.how{color:var(--muted);font-size:14px}.src{margin-top:2px;font-size:12px}ul.ex{margin:4px 0;padding-left:18px}
.okc{color:var(--ok)}.failc{color:var(--fail)}.warnc{color:var(--warn)}details{margin-top:16px;border-top:1px solid var(--line);padding-top:10px}summary{cursor:pointer;font-weight:600}
"#;

pub type Orphans = IndexMap<String, Vec<String>>;

fn orphan_html(orphans: &Orphans) -> String {
    if orphans.is_empty() {
        return String::new();
    }
    let items: String = orphans
        .iter()
        .flat_map(|(n, es)| es.iter().map(move |e| format!("<li><code>{}</code>: {}</li>", escape(n), escape(e))))
        .collect();
    format!(
        "<section class='card'><h2>Orphaned why entries</h2><p class='failc'>These explain definitions that no longer \
         exist (renamed or deleted?):</p><ul>{}</ul></section>",
        items
    )
}

/// What the host realises and assumes (the page's Host card): effect -> how, endpoints (ctor, method, url, reply,
/// fail), and assumptions.
#[derive(Clone, Debug, Default)]
pub struct HostInfo {
    pub effects: IndexMap<String, String>,
    pub endpoints: Vec<(String, String, String, String, String)>,
    pub assumptions: Vec<String>,
}

/// Fixed templates over the host's own tables; every line is marked *assumed*.
pub fn host_html(host: Option<&HostInfo>) -> String {
    let Some(host) = host else { return String::new() };
    let rows: String = host
        .effects
        .iter()
        .map(|(e, how)| format!("<tr><td><code>{}</code></td><td>{}</td></tr>", escape(e), escape(how)))
        .collect();
    let eps: String = host
        .endpoints
        .iter()
        .map(|(c, m, u, r, f)| {
            format!(
                "<li><code>{}</code> → {} <code>{}</code>; reply <code>{}</code>, failure <code>{}</code></li>",
                escape(c),
                escape(m),
                escape(u),
                escape(r),
                escape(f)
            )
        })
        .collect();
    let asm: String =
        host.assumptions.iter().map(|a| format!("<li><span class='badge warn'>assumed</span> {}</li>", escape(a))).collect();
    format!(
        "<section class='card'><header><h2>Host</h2><span class='badge warn'>assumed, not proven</span></header><p \
         class='muted'>The proofs above are about the Kern code. This section lists what the host does with each \
         effect the module can name, and what the proofs assume about it. None of it is checked by the \
         prover.</p><h3>Effects and how this host realises them</h3><table>{}</table>{}<h3>Assumptions</h3><ul>{}</ul></section>",
        rows,
        if eps.is_empty() {
            String::new()
        } else {
            format!("<h3>Network endpoints (the only ones the module can reach)</h3><ul>{}</ul>", eps)
        },
        asm
    )
}

/// The assumed handlers the module rests on: (name, header, spec, whether it can be met).
pub fn assumption_lines(reports: &IndexMap<String, Report>, defs: &Module) -> Vec<(String, String, String, String)> {
    let Some(r) = reports.values().next() else { return vec![] };
    defs.handlers
        .iter()
        .map(|(name, h)| {
            let mut parts: Vec<String> = h
                .needs
                .iter()
                .filter_map(|n| if let Need::Need(c, _, _) = n { Some(format!("need {}", code(c))) } else { None })
                .collect();
            parts.extend(h.ensures.iter().map(|c| format!("ensure {}", code(c))));
            (
                name.clone(),
                h.src_lines["header"][0].clone(),
                parts.join("; "),
                r.handler_sat.get(name).cloned().unwrap_or_else(|| "not checked".into()),
            )
        })
        .collect()
}

pub fn page(reports: &IndexMap<String, Report>, defs: &Module, whys: &IndexMap<String, Status>, title: &str,
            orphans: &Orphans, host: Option<&HostInfo>) -> String {
    let lines = assumption_lines(reports, defs);
    let top = if lines.is_empty() {
        String::new()
    } else {
        let items: String = lines
            .iter()
            .map(|(_, h, sp, sat)| {
                format!(
                    "<li><code>{}</code>: <code>{}</code> <span class='{}'>(spec {})</span></li>",
                    escape(h),
                    escape(sp),
                    if sat.starts_with("unsatisfiable") { "failc" } else { "muted" },
                    escape(sat)
                )
            })
            .collect();
        format!(
            "<section class='card'><h2>Assumptions</h2><p class='muted'>Observations this module rests on. Guarantees \
             marked ⊙ are proven <em>assuming</em> these specs; the prover can't check them, the host checks them \
             on every call.</p><ul>{}</ul></section>",
            items
        )
    };
    let cards: Vec<String> = reports.iter().map(|(n, r)| card(r, defs, &whys[n])).collect();
    let cards = format!("{}{}{}{}", top, cards.join("\n"), orphan_html(orphans), host_html(host));
    format!(
        "<!doctype html><html><head><meta charset='utf-8'><meta name='viewport' \
         content='width=device-width,initial-scale=1'><title>Kern proof: {}</title><style>{}</style></head><body><main><h1>Proof \
         page · {}</h1><p class='sub'>Generated from verifier results by fixed templates — no text here was written \
         by a model.</p>{}</main></body></html>",
        escape(title),
        CSS,
        escape(title),
        cards
    )
}

pub fn terminal(reports: &IndexMap<String, Report>, defs: &Module, whys: &IndexMap<String, Status>,
                orphans: &Orphans) -> String {
    let mut out: Vec<String> = vec![];
    for (_, header, spec, sat) in assumption_lines(reports, defs) {
        out.push(format!(
            "ASSUMES {}: {}   (spec {}; the host provides it and checks it on every call)",
            &header["handler ".len()..],
            spec,
            sat
        ));
    }
    if !out.is_empty() {
        out.push(String::new());
    }
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut last: Option<&String> = None;
    for (n, r) in reports {
        let (_, label) = status(r);
        if let Some(m) = &r.def.module {
            // a package: say which file each block is from
            if last != Some(m) {
                out.push(format!("== {}  ({})", m, relpath(r.def.file.as_deref().unwrap_or(""), &cwd)));
                last = Some(m);
            }
        }
        out.push(format!("{} [{}]  {}", n, r.hash, label));
        for g in &r.guarantees {
            let mut line = format!("  {} {} {}", icon(g.status), g.kind, g.clause);
            if matches!(g.status, G::Refuted | G::Unconfirmed) {
                let cx = g.counterexample.as_ref().expect("a refuted goal has a counterexample");
                line.push_str(&format!("\n      counterexample: {}", cx.eg));
                if !cx.confirmed_by_interpreter {
                    line.push_str("   (not reproduced in the interpreter)");
                }
                if g.kind == "emit" {
                    line.push_str(&format!("\n      audit log got: {}", cx.emitted));
                }
            }
            match (g.status, g.reason.as_deref()) {
                (G::Unknown, Some(reason)) if !reason.is_empty() => {
                    line.push_str(&format!("\n      cannot be proven: {}", reason))
                }
                (G::Vacuous, reason) => line.push_str(&format!("\n      holds only vacuously: {}", reason.unwrap_or(""))),
                _ => {}
            }
            if g.status == G::Tested {
                line.push_str(&format!("   (not proven; held in {} random successes)", g.tested.unwrap_or(0)));
            }
            if g.status == G::Assumed {
                line.push_str(&format!("   (assuming {})", g.assuming.clone().unwrap_or_default().join(", ")));
            }
            if g.status == G::Conditional {
                line.push_str(&format!(
                    "   (relies on {}, which is not proven)",
                    g.blocked_by.clone().unwrap_or_default().join(", ")
                ));
                if g.via_broken_spec == Some(true) {
                    line.push_str("\n      refuted only via that broken spec; not reproduced in the interpreter");
                }
            }
            out.push(line);
        }
        if !r.writes.is_empty() {
            let silent = unguaranteed_writes(r);
            let note = if silent.is_empty() {
                String::new()
            } else {
                format!(
                    "   (no guarantee about the new {} of {})",
                    if all_dotted(&silent) { "value" } else { "balance" },
                    silent.join(", ")
                )
            };
            out.push(format!("  writes {}{}", fmt_writes(&r.writes, &r.def), note));
        }
        for ch in loop_event_note(r, defs) {
            out.push(format!(
                "  {}: events emitted inside a loop are not described by any spec (no per-iteration emit form yet)",
                ch
            ));
        }
        let ex = &r.examples;
        out.push(format!(
            "  examples {}/{}   random tests {}/{} reached success, {} violations",
            ex.iter().filter(|e| e.pass).count(),
            ex.len(),
            r.props.succeeded,
            r.props.runs,
            r.props.violations.len()
        ));
        if let Some(note) = &r.mutation_note {
            out.push(format!("  spec strength: {}", note));
        }
        if let Some(m) = &r.mutation {
            out.push(format!(
                "  spec strength: guarantees {}/{} (prover {}, random tests {}); examples {}/{}; together {}/{}",
                m.guarantee_killed,
                m.total,
                m.prover_killed,
                m.props_killed,
                m.example_killed,
                m.total,
                m.killed,
                m.total
            ));
            for (s, ex) in &m.guarantee_missed {
                out.push(format!(
                    "      {}: {}{}",
                    if *ex { "missed" } else { "survived" },
                    s,
                    if *ex { "   (an example catches it)" } else { "" }
                ));
            }
            if let Some(sm) = &r.spec_mutation {
                out.push(format!(
                    "  spec refusals: {}/{} changes caught (examples {}, guarantees {})",
                    sm.by_examples + sm.by_guarantees,
                    sm.total,
                    sm.by_examples,
                    sm.by_guarantees
                ));
                for (n, cs) in &sm.survived {
                    out.push(format!("      unpinned: {n} ({})", cs.join(", ")));
                }
                for e in &sm.ill_formed {
                    out.push(format!("      ill-formed (not counted): {e}"));
                }
                for e in &sm.errors {
                    out.push(format!("      crashed (not counted): {e}"));
                }
            }
            for e in &m.errors {
                out.push(format!("      checker error: {}", e));
            }
        }
        for x in &r.regression {
            if let Some(rest) = x.strip_prefix("spec changed: ") {
                out.push(format!("  SPEC CHANGED SINCE BASELINE: {}", rest));
            } else if x.starts_with("spec changed") {
                out.push(format!("  SPEC CHANGED SINCE BASELINE: {}", &x["spec changed: ".len().min(x.len())..]));
            } else {
                out.push(format!("  WEAKER THAN BASELINE: {}", x));
            }
        }
        let w = &whys[n];
        if w.count > 0 {
            out.push(format!(
                "  why {}{}",
                w.count,
                if w.stale { "  STALE (definition or its interface changed since written)" } else { "" }
            ));
            for e in &w.obligations {
                out.push(format!("      why reads like an obligation (move it to the spec?): {}", e));
            }
        }
        out.push(String::new());
    }
    for (n, es) in orphans {
        out.push(format!(
            "ORPHANED why: {} entr{} for `{}`, which does not exist",
            es.len(),
            if es.len() == 1 { "y" } else { "ies" },
            n
        ));
    }
    out.join("\n")
}

/// `e - x + x` and `e + x - x` as `e`: exact on Int, so the page may show the shorter form of a lifted condition
/// (a debit and a credit of the same amount on one key).
fn simplify(e: &crate::syntax::Expr) -> crate::syntax::Expr {
    use crate::syntax::Expr;
    match e {
        Expr::Bin(op, l, r) => {
            let (l, r) = (simplify(l), simplify(r));
            if let Expr::Bin(op2, ll, lr) = &l {
                if (*op, *op2) == ("+", "-") || (*op, *op2) == ("-", "+") {
                    if **lr == r {
                        return (**ll).clone();
                    }
                }
            }
            Expr::Bin(op, Box::new(l), Box::new(r))
        }
        Expr::App(n, p, args) => Expr::App(n.clone(), *p, args.iter().map(simplify).collect()),
        _ => e.clone(),
    }
}

/// S8's line on the page: changes to the spec's own needs, and what caught them.
fn spec_mutation_html(sm: &crate::report::SpecMutation) -> String {
    let mut out = format!(
        "<p>Refusals in the spec: {}/{} changes caught, by examples {}, by guarantees {}. <span class='muted'>Each \
         spec <code>need</code> dropped, or its comparison flipped. These measure whether anything pins the refusal, \
         not the body; callers' proofs aren't re-run.</span></p>",
        sm.by_examples + sm.by_guarantees,
        sm.total,
        sm.by_examples,
        sm.by_guarantees
    );
    if !sm.survived.is_empty() {
        let items: String =
            sm.survived.iter().map(|(n, cs)| format!("<li><code>{}</code>: {}</li>", escape(n), ticks_to_code(&cs.join("; ")))).collect();
        out.push_str(&format!(
            "<p>Nothing catches these changes to the spec's refusals. An example at the refusal's boundary would:</p>\
             <ul class='ex'>{items}</ul>"
        ));
    }
    if !sm.ill_formed.is_empty() {
        let items: String = sm.ill_formed.iter().map(|e| format!("<li>{}</li>", ticks_to_code(e))).collect();
        out.push_str(&format!("<p class='muted'>Not counted, since they don't pass the type check:</p><ul class='ex'>{items}</ul>"));
    }
    if !sm.errors.is_empty() {
        let items: String = sm.errors.iter().map(|e| format!("<li>{}</li>", escape(e))).collect();
        out.push_str(&format!("<p class='failc'>The checker crashed on these (not counted):</p><ul class='ex'>{items}</ul>"));
    }
    out
}

/// Escaped text with `quoted` parts as <code>.
fn ticks_to_code(s: &str) -> String {
    s.split('`').enumerate().map(|(i, part)| if i % 2 == 1 { format!("<code>{}</code>", escape(part)) } else { escape(part) }).collect()
}
