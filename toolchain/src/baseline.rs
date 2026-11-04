//! Ratchet baseline (port of kern/baseline.py): the spec should only get stronger.
//!
//! `kern baseline FILE.k` records, per definition, its guarantee strength and its guarantee clauses. `kern check`
//! compares against it and flags a definition whose guarantee strength fell or whose guarantees were removed.
//! Default file: <module>.strength.
//!
//! File format:
//!     transfer 9/13
//!       keep ledger.total
//!       ensure ledger.balance' from = ledger.balance from - amount

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::report::Report;
use crate::syntax::{code, stmt_code, Module, Need, Stmt};
use crate::views::body_needs;

pub const V2: &str = "# kern baseline v2: guarantees, spec needs and own body-level refusals";

fn head() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(\w+) (\d+)/(\d+)$").unwrap())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub killed: u64,
    pub total: u64,
    pub clauses: Vec<String>,
    pub refusals: Vec<String>,
    pub v2: bool,
}

pub type Baseline = IndexMap<String, Entry>;

pub fn clauses(r: &Report) -> Vec<String> {
    r.guarantees.iter().map(|g| format!("{} {}", g.kind, g.clause)).collect()
}

/// What a caller relies on about *when* the call refuses: the spec needs and the definition's own body-level
/// conditions (inferred, `body need ...`). Callees' conditions belong to the callees.
pub fn refusals(r: &Report, defs: &Module) -> Vec<String> {
    let need_stmt = |n: &Need| match n {
        Need::Need(c, k, a) => Some(stmt_code(&Stmt::Need(c.clone(), k.clone(), a.clone()))),
        Need::As(..) => None,
    };
    let mut out: Vec<String> = r.def.needs.iter().filter_map(need_stmt).collect();
    out.extend(body_needs(r));
    for a in &r.assumptions {
        // a proof that rests on an assumed spec
        if let Some(h) = defs.handlers.get(a) {
            out.extend(h.needs.iter().filter_map(need_stmt).map(|s| format!("need assumes {}: {}", a, s)));
            out.extend(h.ensures.iter().map(|c| format!("need assumes {}: ensure {}", a, code(c))));
        }
    }
    out
}

/// The baseline file's text for these reports (only clean, measured definitions set a baseline).
pub fn text(reports: &IndexMap<String, Report>, defs: &Module) -> String {
    let mut out = format!("{}\n", V2);
    for (n, r) in reports {
        let Some(m) = &r.mutation else { continue };
        out.push_str(&format!("{} {}/{}\n", n, m.guarantee_killed, m.total));
        for c in clauses(r).into_iter().chain(refusals(r, defs)) {
            out.push_str(&format!("  {}\n", c));
        }
    }
    out
}

pub fn write(path: &str, reports: &IndexMap<String, Report>, defs: &Module) -> std::io::Result<()> {
    std::fs::write(path, text(reports, defs))
}

pub fn load(path: &str) -> Baseline {
    if path.is_empty() {
        return Baseline::new();
    }
    match crate::package::read_text(path) {
        Ok(t) => parse(&t),
        Err(_) => Baseline::new(),
    }
}

/// A baseline from its text. Old files (no v2 header) recorded guarantees only, so their needs are not compared
/// (that would flag every need as new).
pub fn parse(text: &str) -> Baseline {
    let mut base = Baseline::new();
    let lines = crate::syntax::splitlines(text);
    let v2 = lines.first().map_or(false, |l| l.trim() == V2);
    let mut cur: Option<String> = None;
    for line in lines {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let first_space = line.chars().next().map_or(false, |c| c.is_whitespace());
        match head().captures(line.trim()) {
            Some(m) if !first_space => {
                let name = m[1].to_string();
                base.insert(
                    name.clone(),
                    Entry {
                        killed: m[2].parse().unwrap_or(0),
                        total: m[3].parse().unwrap_or(0),
                        clauses: vec![],
                        refusals: vec![],
                        v2,
                    },
                );
                cur = Some(name);
            }
            _ => {
                if let Some(c) = &cur {
                    let e = base.get_mut(c).unwrap();
                    let t = line.trim().to_string();
                    if t.starts_with("need ") || t.starts_with("body need ") {
                        e.refusals.push(t);
                    } else {
                        e.clauses.push(t);
                    }
                }
            }
        }
    }
    base
}

/// Set r.regression (list of messages) on every report weaker than its baseline.
pub fn compare(reports: &mut IndexMap<String, Report>, defs: &Module, base: &Baseline) {
    for (n, r) in reports.iter_mut() {
        r.regression = vec![];
        let Some(b) = base.get(n) else { continue };
        let now_clauses = clauses(r);
        let gone: Vec<String> = b.clauses.iter().filter(|c| !now_clauses.contains(c)).cloned().collect();
        if !gone.is_empty() {
            r.regression.push(format!("guarantees removed since the baseline: {}", gone.join("; ")));
        }
        if b.v2 {
            // when the call refuses is part of what callers rely on
            let now = refusals(r, defs);
            let dropped: Vec<String> = b.refusals.iter().filter(|c| !now.contains(c)).cloned().collect();
            let added: Vec<String> = now.iter().filter(|c| !b.refusals.contains(c)).cloned().collect();
            if !dropped.is_empty() {
                r.regression.push(format!(
                    "spec changed: refusals removed or changed since the baseline (it may now accept what it \
                     refused): {}",
                    dropped.join("; ")
                ));
            }
            if !added.is_empty() {
                r.regression.push(format!(
                    "spec changed: new refusals since the baseline (it may now refuse what it accepted): {}",
                    added.join("; ")
                ));
            }
        }
        if let Some(m) = &r.mutation {
            if b.total > 0 && m.guarantee_killed * b.total < b.killed * m.total {
                r.regression.push(format!(
                    "guarantee strength fell from {}/{} to {}/{}",
                    b.killed, b.total, m.guarantee_killed, m.total
                ));
            }
        } else if !r.clean && b.total > 0 {
            r.regression.push("was clean at the baseline, is not now".into());
        }
    }
}
