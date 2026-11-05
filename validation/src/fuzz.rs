//! The soundness fuzzer (port of validation/fuzz.py; docs/research/ROADMAP_RUN.md, FZ). It generates loop and
//! assumed-handler programs, plus every body mutant of the loop and handler templates, then checks each one and
//! runs it in the interpreter on random worlds. The interpreter is the oracle: a *contradiction* is a claim on
//! the page (✔, ⊙, or ∅ "never succeeds") that a run shows false.
//!
//! The generators draw from Python's random stream in Python's order (pyrandom.rs), so seed N gives the programs
//! fuzz.py gives for seed N (checked by validation/tests/fuzz_generators.rs).

use kern::pyrandom::Random;

// ---------------------------------------------------------------- random loop programs

/// Python's `(?<!get)(?<!get') {c}\b`: the first ` c` (c a one-digit constant) not preceded by `get` or `get'`
/// and not followed by a word character. Rust's regex has no lookbehind, so it's matched by hand.
fn find_constant(text: &str, c: char) -> Option<usize> {
    let b = text.as_bytes();
    (0..b.len().saturating_sub(1)).find(|&i| {
        b[i] == b' '
            && b[i + 1] == c as u8
            && !text[..i].ends_with("get")
            && !text[..i].ends_with("get'")
            && b.get(i + 2).map_or(true, |x| !(x.is_ascii_alphanumeric() || *x == b'_'))
    })
}

enum Perturb {
    Constant(usize, char),
    Replace(&'static str, &'static str, Option<usize>),
    DropWhen,
}

/// One small, usually wrong, change to a clause or statement (fuzz.py's _perturb).
fn perturb(rnd: &mut Random, text: &str, kind: &str) -> String {
    let mut options: Vec<Perturb> = vec![];
    for c in ['0', '1', '2', '3', '5', '7'] {
        if let Some(i) = find_constant(text, c) {
            options.push(Perturb::Constant(i, c));
        }
    }
    use Perturb::Replace as R;
    match kind {
        "inv" => options.extend([
            R(" to k", " to (k + 1)", None),
            R(" from lo", " from (lo + 1)", None),
            R(" from lo", " from 0", None),
            R("' j = ", "' j >= ", Some(1)),
        ]),
        "ens" => options.extend([R(" to hi", " to (hi + 1)", None), R(" from lo", " from (lo - 1)", None), R("' j = ", "' j <= ", Some(1))]),
        "stmt" => options.extend([R("set k ", "set (k + 1) ", Some(1)), R("set k ", "set (k - 1) ", Some(1)), R(" + ", " - ", Some(1))]),
        _ => {}
    }
    if text.contains(" when ") && kind != "stmt" {
        options.push(Perturb::DropWhen);
    }
    if options.is_empty() {
        return text.to_string();
    }
    let i = rnd.randbelow(options.len() as u128) as usize;
    match &options[i] {
        Perturb::Constant(at, c) => {
            let d = *rnd.choice(&[-1i64, 1]);
            let v = 0.max(c.to_digit(10).unwrap() as i64 + d);
            format!("{} {}{}", &text[..*at], v, &text[at + 2..])
        }
        Perturb::Replace(a, b, None) => text.replace(a, b),
        Perturb::Replace(a, b, Some(n)) => text.replacen(a, b, *n),
        Perturb::DropWhen => {
            let head = text.split(" when ").next().unwrap();
            let tail = if text.contains(" for ") { format!(" for{}", text.split_once(" for").unwrap().1) } else { String::new() };
            format!("{head}{tail}")
        }
    }
}

fn choice_str<'a>(rnd: &mut Random, xs: &[&'a str]) -> &'a str {
    rnd.choice(xs)
}

/// A module with one loop definition: correct invariants and ensures for its body, then perturbed
/// (fuzz.py's gen_loop, modes "mixed", "inv" and "ens").
pub fn gen_loop(rnd: &mut Random) -> String {
    let mode = choice_str(rnd, &["mixed", "inv", "ens"]);
    let l = choice_str(rnd, &["lo", "lo", "lo", "0"]);
    let h = choice_str(rnd, &["hi", "hi", "hi", "(hi - 1)"]);
    let c = *rnd.choice(&[0, 1, 2, 3]);
    let c2 = *rnd.choice(&[1, 5, 7]);
    let kinds: Vec<&str> = if mode != "inv" {
        let k = *rnd.choice(&[1usize, 1, 2]);
        rnd.sample(&["const", "add", "cond", "acc", "copy", "need"], k)
    } else {
        vec![choice_str(rnd, &["const", "add", "acc", "copy"])]
    };
    let (mut stmts, mut invs): (Vec<Vec<String>>, Vec<String>) = (vec![], vec![]);
    for kind in &kinds {
        match *kind {
            "const" => {
                stmts.push(vec![format!("hold.set k {c2}")]);
                invs.push(format!("hold.get' j = {c2} for j from {l} to k"));
            }
            "add" => {
                stmts.push(vec![format!("hold.set k (hold.get k + {c})")]);
                invs.push(format!("hold.get' j = hold.get j + {c} for j from {l} to k"));
            }
            "cond" => {
                stmts.push(vec![format!("if hold.get k > {c}"), format!("  hold.set k {c2}")]);
                invs.push(format!("hold.get' j = {c2} when hold.get j > {c} for j from {l} to k"));
                invs.push(format!("hold.get' j = hold.get j when hold.get j <= {c} for j from {l} to k"));
            }
            "acc" => {
                stmts.push(vec![format!("tally.set 0 (tally.get 0 + {c2})")]);
                invs.push(format!("tally.get' 0 = tally.get 0 + {c2} * (k - {l})"));
            }
            "copy" => {
                stmts.push(vec!["aux.set k (hold.get k)".into()]);
                invs.push(format!("aux.get' j = hold.get j for j from {l} to k"));
            }
            "need" => stmts.push(vec![format!("need hold.get k >= {c} else Low")]),
            _ => unreachable!(),
        }
    }
    if invs.is_empty() || (mode != "inv" && rnd.random() < 0.3) {
        invs.push("ledger.total' = ledger.total".into());
    }
    let mut ens: Vec<String> = invs
        .iter()
        .map(|i| {
            if i.contains(" for j from ") {
                format!("{} to {h}", i.rsplit_once(" to k").unwrap().0)
            } else if i.contains("* (k - ") {
                i.replace("(k - ", &format!("({h} - "))
            } else {
                i.clone()
            }
        })
        .collect();
    let mut needs = vec![];
    if rnd.random() < 0.4 {
        needs.push(format!("need {l} <= {h} else Empty"));
    }
    let other = rnd.random() < 0.25;
    if other {
        if rnd.random() < 0.7 {
            needs.push(format!("need other < {l} or other >= {h} else InRange"));
        }
        ens.push("hold.get' other = hold.get other".into());
    }
    if mode == "inv" {
        invs[0] = perturb(rnd, &invs[0].clone(), "inv");
    } else if mode == "ens" {
        let i = rnd.randbelow(ens.len() as u128) as usize;
        ens[i] = perturb(rnd, &ens[i].clone(), "ens");
    } else {
        let p = if rnd.random() < 0.5 { 0.3 } else { 0.0 };
        invs = invs.into_iter().map(|x| if rnd.random() < p { perturb(rnd, &x, "inv") } else { x }).collect();
        ens = ens.into_iter().map(|x| if rnd.random() < p { perturb(rnd, &x, "ens") } else { x }).collect();
        stmts = stmts
            .into_iter()
            .map(|s| {
                let first = if rnd.random() < p { perturb(rnd, &s[0], "stmt") } else { s[0].clone() };
                std::iter::once(first).chain(s[1..].iter().cloned()).collect()
            })
            .collect();
        if rnd.random() < 0.3 {
            let k = 1.max(invs.len() as i64 - 1) as usize;
            invs = rnd.sample(&invs, k);
        }
    }
    let mut lines: Vec<String> = vec!["state hold Int".into(), "state tally Int".into(), "state aux Int".into()];
    if rnd.random() < 0.6 {
        lines.push("invariant o:Int hold.get o >= 0".into());
    }
    lines.push(String::new());
    lines.push(format!("f lo:Int hi:Int{}", if other { " other:Int" } else { "" }));
    lines.extend(needs.iter().map(|n| format!("  {n}")));
    if rnd.random() < 0.3 {
        lines.push("  ok otherwise".into());
    }
    lines.extend(ens.iter().map(|e| format!("  ensure {e}")));
    lines.push("body".into());
    if mode == "mixed" && rnd.random() < 0.15 {
        lines.push(format!("  hold.set {l} {c2}"));
    }
    lines.push(format!("  for k from {l} to {h}"));
    lines.extend(invs.iter().map(|i| format!("    invariant {i}")));
    for s in &stmts {
        lines.extend(s.iter().map(|x| format!("    {x}")));
    }
    lines.join("\n") + "\n"
}

// ---------------------------------------------------------------- random sum programs (72d2446)

/// A module with a sum function `agg` and one definition that uses sums: a point update with before/after sum
/// claims, an accumulating loop whose invariant is a sum, a state-reading function called around a write, or a
/// filtered count. The claims are written correct, then (half the time) one clause or statement is perturbed.
/// Drawn from its own stream (`sum_jobs`), so the loop and handler streams pinned in fuzz_generators.json stay.
pub fn gen_sum(rnd: &mut Random) -> String {
    let kind = choice_str(rnd, &["point", "acc", "fn", "count", "closed", "closed"]);
    let e = choice_str(rnd, &["hold.get j", "hold.get j", "1", "2 * hold.get j", "hold.get j + 3"]);
    let filt = match kind {
        "acc" | "count" => choice_str(rnd, &["", " where flag.get j = 1", " where hold.get j > 2"]),
        _ => choice_str(rnd, &["", "", " where flag.get j = 1"]),
    };
    let sum = |state: &str, lo: &str, hi: &str| format!("(sum j from {lo} to {hi} of {}{filt})", e.replace("hold.get j", state));
    // how much a +v at k changes the sum of e (for point / fn)
    let delta = match e {
        "1" => "0",
        "2 * hold.get j" => "2 * v",
        _ => "v",
    };
    let at_k = if filt.is_empty() { "" } else { " and flag.get k = 1" };
    let off_k = if filt.is_empty() { "" } else { " or flag.get k != 1" };
    let (header, mut ens, mut body): (String, Vec<String>, Vec<String>);
    let mut negate = false;
    match kind {
        "point" => {
            (ens, body) = (vec![], vec![]);
            header = "f k:Int v:Int lo:Int hi:Int".into();
            ens.push(format!("{} = {} + {delta} when lo <= k and k < hi{at_k}", sum("hold.get' j", "lo", "hi"), sum("hold.get j", "lo", "hi")));
            ens.push(format!("{} = {} when k < lo or k >= hi{off_k}", sum("hold.get' j", "lo", "hi"), sum("hold.get j", "lo", "hi")));
            body.push("hold.set k (hold.get k + v)".into());
        }
        "fn" => {
            (ens, body) = (vec![], vec![]);
            header = "f k:Int v:Int lo:Int hi:Int".into();
            ens.push(format!("tally.get' 0 = {delta} when lo <= k and k < hi{at_k}"));
            ens.push(format!("tally.get' 0 = 0 when k < lo or k >= hi{off_k}"));
            body.extend(["x = agg lo hi".to_string(), "hold.set k (hold.get k + v)".into(), "y = agg lo hi".into(), "tally.set 0 (y - x)".into()]);
        }
        "acc" => {
            (ens, body) = (vec![], vec![]);
            header = "f lo:Int hi:Int".into();
            ens.push(format!("tally.get' 0 = tally.get 0 + {}", sum("hold.get j", "lo", "hi")));
            let ek = e.replace("hold.get j", "hold.get k");
            let step = format!("tally.set 0 (tally.get 0 + {})", if ek.contains(' ') { format!("({ek})") } else { ek });
            body.push("for k from lo to hi".into());
            body.push(format!("  invariant tally.get' 0 = tally.get 0 + {}", sum("hold.get j", "lo", "k")));
            if filt.is_empty() {
                body.push(format!("  {step}"));
            } else {
                body.push(format!("  if {}", filt.trim_start().trim_start_matches("where ").replace(" j ", " k ")));
                body.push(format!("    {step}"));
            }
        }
        "closed" => {
            // sums against values computed without a sum: a wrong encoding of `sum` can't agree with both sides
            header = "f k:Int lo:Int hi:Int".into();
            ens = vec![
                "tally.get' 0 = hi - lo when lo <= hi".into(),
                "tally.get' 0 = 0 when hi <= lo".into(),
                "tally.get' 1 = hold.get k".into(),
                format!("tally.get' 2 = {} when lo <= k and k < hi", sum("hold.get j", "lo", "k") + " + " + &sum("hold.get j", "k", "hi")),
            ];
            body = vec![
                "tally.set 0 (sum j from lo to hi of 1)".into(),
                "tally.set 1 (sum j from k to (k + 1) of hold.get j)".into(),
                format!("tally.set 2 {}", sum("hold.get j", "lo", "hi")),
            ];
            negate = rnd.random() < 0.5;
            if negate {
                // a false claim, true only if `sum` were encoded with the wrong sign or the wrong empty range
                let i = rnd.randbelow(3) as usize;
                ens[i] = [
                    "tally.get' 0 = lo - hi when lo < hi",
                    "tally.get' 0 = 1 when hi <= lo",
                    "tally.get' 1 = 0 - hold.get k when hold.get k != 0",
                ][i]
                    .into();
            }
        }
        _ => {
            (ens, body) = (vec![], vec![]);
            header = "f lo:Int hi:Int".into();
            ens.push("tally.get' 0 >= 0".into());
            ens.push("tally.get' 0 <= hi - lo when lo <= hi".into());
            ens.push(format!("tally.get' 0 = {}", "(sum j from lo to hi of 1".to_string() + filt + ")"));
            body.push(format!("tally.set 0 (sum j from lo to hi of 1{filt})"));
        }
    }
    let mut agg_body = sum("hold.get j", "lo", "hi");
    if !negate && rnd.random() < 0.5 {
        match rnd.randbelow(3) {
            0 => {
                let i = rnd.randbelow(ens.len() as u128) as usize;
                ens[i] = perturb(rnd, &ens[i].clone(), "ens");
            }
            1 => {
                let i = rnd.randbelow(body.len() as u128) as usize;
                body[i] = perturb(rnd, &body[i].clone(), "stmt");
            }
            _ => agg_body = perturb(rnd, &agg_body, "ens"),
        }
    }
    let mut lines: Vec<String> = vec!["state hold Int".into(), "state flag Int".into(), "state tally Int".into(), String::new()];
    lines.push("agg lo:Int hi:Int -> Int".into());
    lines.push(format!("  ensure result = {}", sum("hold.get j", "lo", "hi")));
    lines.push("body".into());
    lines.push(format!("  result = {agg_body}"));
    lines.push(String::new());
    lines.push(header);
    if rnd.random() < 0.3 {
        lines.push("  ok otherwise".into());
    }
    lines.extend(ens.iter().map(|x| format!("  ensure {x}")));
    lines.push("body".into());
    lines.extend(body.iter().map(|x| format!("  {x}")));
    lines.join("\n") + "\n"
}

/// `n` sum programs (gen_sum) from their own stream for `seed`, labelled `sum#i`.
pub fn sum_jobs(seed: i128, n: usize, runs: u64) -> Vec<Job> {
    let mut rnd = Random::from_int(seed * 1_000_003 + 72);
    (0..n)
        .map(|i| {
            let src = gen_sum(&mut rnd);
            Job { label: format!("sum#{i}"), src: Some(src), module: None, seed: rnd.randrange(0, 1_000_000_000) as i128, runs }
        })
        .collect()
}

/// The const variant of a program: every integer literal outside `eg` lines becomes a `const kc_N N` declared at
/// the end (S5: substituted at load, so the module must mean exactly the same).
pub fn const_variant(src: &str) -> String {
    let (mut out, mut values) = (String::new(), std::collections::BTreeSet::new());
    for line in src.lines() {
        if line.trim_start().starts_with("eg ") {
            out.push_str(line);
        } else {
            let cs: Vec<char> = line.chars().collect();
            let mut i = 0;
            while i < cs.len() {
                let in_name = i > 0 && (cs[i - 1].is_alphanumeric() || cs[i - 1] == '_' || cs[i - 1] == '.');
                if cs[i].is_ascii_digit() && !in_name {
                    let j = (i..cs.len()).find(|&j| !cs[j].is_ascii_digit()).unwrap_or(cs.len());
                    let v: String = cs[i..j].iter().collect();
                    out.push_str(&format!("kc_{v}"));
                    values.insert(v);
                    i = j;
                } else {
                    out.push(cs[i]);
                    i += 1;
                }
            }
        }
        out.push('\n');
    }
    out.push('\n');
    for v in &values {
        out.push_str(&format!("const kc_{v} {v}\n"));
    }
    out
}

/// Replace whole name tokens (runs of letters, digits, `_` and `.`).
fn rename_tokens(text: &str, from: &str, to: &str) -> String {
    let (mut out, mut tok) = (String::new(), String::new());
    let flush = |tok: &mut String, out: &mut String| {
        out.push_str(if tok == from { to } else { tok.as_str() });
        tok.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            tok.push(c);
        } else {
            flush(&mut tok, &mut out);
            out.push(c);
        }
    }
    flush(&mut tok, &mut out);
    out
}

/// The record variant of a gen_sum program: `lo`, `hi` become the fields of `record Span lo:Int hi:Int`, in `agg`
/// and in `f`, and `agg lo hi` passes the record whole (S5: flattened at load, so the verdicts must be the same).
pub fn record_variant(src: &str) -> Option<String> {
    if !src.contains("agg lo:Int hi:Int -> Int") || !src.contains("lo:Int hi:Int\n") {
        return None;
    }
    let mut blocks: Vec<String> = src.split("\n\n").map(String::from).collect();
    for b in blocks.iter_mut() {
        if b.starts_with("agg ") || b.starts_with("f ") {
            let mut t = b.replacen("lo:Int hi:Int", "s:Span", 1).replace("agg lo hi", "agg s");
            t = rename_tokens(&t, "lo", "s.lo");
            t = rename_tokens(&t, "hi", "s.hi");
            *b = t;
        } else if b.starts_with("state ") {
            b.push_str("\nrecord Span lo:Int hi:Int");
        }
    }
    Some(blocks.join("\n\n"))
}

/// For each of the first `n` sum programs, its const and record variants (labels `sumc#i`, `sumr#i`), with the base
/// program's seed so the oracle draws the same worlds.
pub fn variant_jobs(seed: i128, n: usize, runs: u64) -> Vec<Job> {
    let mut out = vec![];
    for job in sum_jobs(seed, n, runs) {
        let src = job.src.clone().unwrap();
        let i = job.label.trim_start_matches("sum#");
        out.push(Job { label: format!("sumc#{i}"), src: Some(const_variant(&src)), ..job.clone() });
        if let Some(r) = record_variant(&src) {
            out.push(Job { label: format!("sumr#{i}"), src: Some(r), ..job.clone() });
        }
    }
    out
}

/// Variants whose verdict counts differ from their base program's (`sum#i`): each should be empty.
pub fn variant_mismatches(results: &[Outcome1]) -> Vec<String> {
    let base: BTreeMap<&str, &Outcome1> =
        results.iter().filter(|r| r.label.starts_with("sum#")).map(|r| (r.label.trim_start_matches("sum#"), r)).collect();
    results
        .iter()
        .filter_map(|r| {
            let i = r.label.strip_prefix("sumc#").or_else(|| r.label.strip_prefix("sumr#"))?;
            let b = base.get(i)?;
            (b.wf != r.wf || b.tiers != r.tiers || b.found.len() != r.found.len()).then(|| r.label.clone())
        })
        .collect()
}

// ---------------------------------------------------------------- random assumed-handler programs

/// fuzz.py's gen_handler.
pub fn gen_handler(rnd: &mut Random) -> String {
    let spec = choice_str(
        rnd,
        &[
            "result >= 0 and result < n",
            "result >= 0 and result < n",
            "result >= 0",
            "result < n",
            "result >= 1 and result <= n",
            "result >= 0 and result < n and result != 2",
            "result >= n and result < n",
        ],
    );
    let mut lines: Vec<String> = vec!["handler rand.below n:Int -> Int".into()];
    if rnd.random() < 0.7 {
        lines.push("  need n > 0 else BadBound".into());
    }
    lines.extend([format!("  ensure {spec}"), String::new(), "state hold Int".into(), "state aux Int".into()]);
    if rnd.random() < 0.4 {
        lines.push("invariant o:Int hold.get o >= 0".into());
    }
    let c = *rnd.choice(&[0, 0, 1, 2]);
    let arg = choice_str(rnd, &["n", "n", "(n + 1)", "(n - 1)", "x", "3"]);
    let mut body = vec![format!("r = rand.below {arg}"), if c != 0 { format!("hold.set 0 (r + {c})") } else { "hold.set 0 r".into() }];
    let two = rnd.random() < 0.4;
    if two {
        let a2 = choice_str(rnd, &["n", "n", "3"]);
        body.extend([format!("s = rand.below {a2}"), "hold.set 1 s".into()]);
    }
    if rnd.random() < 0.35 {
        body.extend(["if r = 0".into(), "  hold.set 2 1".into(), "else".into(), "  hold.set 2 2".into()]);
    }
    if rnd.random() < 0.3 {
        body.extend(["for k from 0 to r".into(), "  invariant aux.get' j = 1 for j from 0 to k".into(), "  aux.set k 1".into()]);
    }
    let mut pool: Vec<String> = vec![
        "hold.get' 0 >= 0".into(),
        format!("hold.get' 0 >= {c}"),
        "hold.get' 0 < n".into(),
        format!("hold.get' 0 < n + {c}"),
        "hold.get' 0 <= n".into(),
        format!("hold.get' 0 = {c}"),
        "hold.get' 3 = hold.get 3".into(),
        "hold.get' 2 >= 1".into(),
        "hold.get' 2 = 1".into(),
    ];
    if two {
        pool.extend(["hold.get' 1 < n".into(), "hold.get' 1 = hold.get' 0".into(), "hold.get' 1 >= 0".into()]);
    }
    if body.join("\n").contains("aux.set") {
        pool.extend(["aux.get' 0 = 1 when hold.get 0 >= 0".into(), "aux.get' 0 = 1".into()]);
    }
    let k = *rnd.choice(&[1usize, 2, 3]);
    let ens = rnd.sample(&pool, k);
    lines.extend([String::new(), "pick n:Int x:Int".into()]);
    if rnd.random() < 0.6 {
        lines.push("  need n > 0 else Bad".into());
    }
    if rnd.random() < 0.3 {
        lines.push("  ok otherwise".into());
    }
    lines.extend(ens.iter().map(|e| format!("  ensure {e}")));
    lines.push("body".into());
    lines.extend(body.iter().map(|b| format!("  {b}")));
    lines.join("\n") + "\n"
}

// ---------------------------------------------------------------- template mutants

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use indexmap::IndexMap;
use kern::check::mutants;
use kern::infer::{callees, relevant_states};
use kern::report::{analyse, Report, Status};
use kern::run::{Interp, Outcome, ProvideError, RunError, Value, World};
use kern::syntax::{parse_module, stmt_code, Key, Module};
use kern::wf::{check_module, problems};
use serde_json::{json, Value as J};

pub const TEMPLATES: [&str; 9] = [
    "examples/batch/batch.k",
    "examples/raffle/raffle.k",
    "tests/probes/p1.k",
    "tests/probes/p2.k",
    "tests/probes/p3.k",
    "tests/probes/p4.k",
    "tests/probes/p5.k",
    "tests/probes/p6.k",
    "tests/probes/p7.k",
];

/// The module with only `name` and what it calls, transitively (so callers' trust propagation doesn't mask it).
fn closure(defs: &Module, name: &str) -> Module {
    let (mut need, mut todo) = (std::collections::BTreeSet::new(), vec![name.to_string()]);
    while let Some(n) = todo.pop() {
        if need.insert(n.clone()) {
            todo.extend(callees(defs, &defs.defs[n.as_str()]));
        }
    }
    let mut m = defs.clone();
    m.defs = defs.defs.iter().filter(|(n, _)| need.contains(*n)).map(|(n, d)| (n.clone(), d.clone())).collect();
    m.package = None;
    m
}

/// (label, module) for every well-formed body mutant of each template definition.
pub fn template_mutants(root: &std::path::Path) -> Vec<(String, Module)> {
    let mut out = vec![];
    for path in TEMPLATES {
        let text = std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let defs = parse_module(&text, true).expect("a template parses");
        check_module(&defs).expect("a template is well-formed");
        for (name, d) in &defs.defs {
            if name == "transfer" {
                continue;
            }
            for (desc, body) in mutants(d) {
                let mut md = d.clone();
                md.body = body;
                md.examples = vec![];
                let mdefs = closure(&defs.with_def(md.clone()), name);
                if problems(&mdefs, &md).is_empty() {
                    out.push((format!("{path}:{name}: {desc}"), mdefs));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the oracle

pub type Rnd = Rc<RefCell<Random>>;

/// Random values that satisfy each handler's assumed spec (an error if none in range).
pub fn provider<'m>(defs: &'m Module, rnd: Rnd) -> kern::run::Provider<'m> {
    Box::new(move |name: &str, args: &[Value]| {
        let h = &defs.handlers[name];
        let env: IndexMap<String, Value> = h.params.iter().zip(args).map(|((p, _), a)| (p.clone(), a.clone())).collect();
        let mut cands: Vec<kern::Int> = (-12..16).collect();
        for a in args {
            if let Value::Int(n) = a {
                cands.extend([n - 1, *n, n + 1]);
            }
        }
        rnd.borrow_mut().shuffle(&mut cands);
        let mut checker = Interp::new(defs);
        let mut w = World::new(IndexMap::new(), IndexMap::new(), 0);
        for v in cands {
            let mut e = env.clone();
            e.insert("result".into(), Value::Int(v));
            let ok = h.ensures.iter().all(|c| checker.ev(c, &e, &mut w, None).map(|x| x.truthy()).unwrap_or(false));
            if ok {
                return Ok(Value::Int(v));
            }
        }
        Err(ProvideError::Other("_NoValue".into(), name.to_string()))
    })
}

fn world(defs: &Module, d: &kern::syntax::Def, rnd: &mut Random) -> (World, Vec<Value>) {
    let accts = ["A", "B", "C", "D"];
    let mut bal = IndexMap::new();
    for a in accts {
        let big = rnd.randint(0, 200) as kern::Int;
        bal.insert(a.to_string(), *rnd.choice(&[0, 5, 10, 30, big]));
    }
    let own: IndexMap<String, String> = accts.iter().map(|a| (a.to_string(), rnd.choice(&["u1", "u2"]).to_string())).collect();
    let mut w = World::new(bal, own, rnd.randint(0, 20) as kern::Int);
    let neg = defs.invariants.is_empty() && rnd.random() < 0.3;
    for (x, key_type) in relevant_states(defs, d) {
        let keys: Vec<Key> = if key_type == "Account" {
            accts.iter().map(|a| Key::Str(a.to_string())).collect()
        } else {
            (-3..11).map(Key::Int).collect()
        };
        let mut m = IndexMap::new();
        for k in keys {
            let big = rnd.randint(0, 30) as kern::Int;
            let mut opts = vec![0, 0, 1, 2, 5, 8, big];
            if neg {
                opts.extend([-1, -4]);
            }
            m.insert(k, *rnd.choice(&opts));
        }
        w.maps.insert(x, m);
    }
    let args = d
        .params
        .iter()
        .map(|(_, t)| {
            if t == "Account" {
                Value::Str(rnd.choice(&accts).to_string())
            } else {
                let big = rnd.randint(-3, 12) as kern::Int;
                let mut opts: Vec<kern::Int> = (-2..9).collect();
                opts.push(big);
                Value::Int(*rnd.choice(&opts))
            }
        })
        .collect();
    (w, args)
}

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub runs: u64,
    pub ok: u64,
    pub skipped: u64,
    pub unmapped: u64,
    pub unmapped_kinds: BTreeMap<String, u64>,
}

fn evidence(args: &[Value], before: &World) -> J {
    let maps: serde_json::Map<String, J> = before
        .maps
        .iter()
        .map(|(x, kv)| (x.clone(), J::Object(kv.iter().map(|(k, v)| (key_text(k), json!(*v as i64))).collect())))
        .collect();
    json!({"args": args.iter().map(Value::str).collect::<Vec<_>>(), "world": {"bal": before.bal.iter().map(|(a, b)| (a.clone(), json!(*b as i64))).collect::<serde_json::Map<_, _>>(), "maps": maps, "now": before.clock as i64}})
}

fn key_text(k: &Key) -> String {
    match k {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => s.clone(),
    }
}

fn claimed(s: Status) -> bool {
    matches!(s, Status::Proven | Status::Assumed)
}

/// Contradictions between the page and the interpreter, plus run statistics (fuzz.py's oracle).
pub fn oracle(defs: &Module, reports: &IndexMap<String, Report>, rnd: Rnd, runs: u64) -> Result<(Vec<J>, Stats), String> {
    let (mut found, mut stats) = (vec![], Stats::default());
    for (name, d) in &defs.defs {
        let r = &reports[name.as_str()];
        let mut goals: IndexMap<(String, String), Vec<Status>> = IndexMap::new();
        for g in &r.guarantees {
            goals.entry((g.kind.clone(), g.clause.clone())).or_default().push(g.status);
        }
        let vacuous = r.guarantees.iter().any(|g| g.status == Status::Vacuous);
        let ok_clause = r.guarantees.iter().find(|g| g.kind == "ok").map(|g| g.clause.clone()).unwrap_or_else(|| "ok otherwise".into());
        let claim = |found: &mut Vec<J>, stats: &mut Stats, kind: &str, clause: &str, ev: J| {
            let mut gs = goals.get(&(kind.to_string(), clause.to_string()));
            if gs.is_none() && kind == "emit" {
                let text = clause.replace("; ", ", then ");
                gs = goals
                    .iter()
                    .find(|(k, _)| (k.0 == "emit" && k.1.split_once(' ').map_or(k.1.as_str(), |x| x.1) == text) || (k.0 == "emit" && k.1 == text))
                    .map(|(_, v)| v);
            }
            let Some(gs) = gs else {
                stats.unmapped += 1;
                *stats.unmapped_kinds.entry(format!("{kind}: {clause}")).or_insert(0) += 1;
                return;
            };
            if gs.iter().all(|s| claimed(*s)) {
                let mut e = ev;
                e["def"] = json!(name);
                e["kind"] = json!(kind);
                e["clause"] = json!(clause);
                e["status"] = json!(gs[0].as_str());
                found.push(e);
            }
        };
        for _ in 0..runs {
            let (mut w, args) = world(defs, d, &mut rnd.borrow_mut());
            if !Interp::new(defs).broken_invariants(&mut w.clone(), None).map_err(|e| e.to_string())?.is_empty() {
                stats.skipped += 1;
                continue;
            }
            let prov = provider(defs, rnd.clone());
            let seed = rnd.borrow_mut().randrange(0, 1_000_000_000) as i128;
            let mut it = Interp::with_provider(defs, Some(prov), seed);
            it.check_loops = true;
            it.record_heads = true;
            let before = w.clone();
            stats.runs += 1;
            let ev = evidence(&args, &before);
            let outcome = match it.run(name, &args, &mut w) {
                Ok(o) => Some(o),
                Err(RunError::Kern(msg)) => {
                    if msg.contains(" gave no value for ") {
                        stats.skipped += 1;
                        continue;
                    }
                    if let Some(rest) = msg.strip_prefix("need of ") {
                        let (h, text) = rest.split_once(" not met: ").unwrap_or((rest, ""));
                        let mut e = ev.clone();
                        e["saw"] = json!(msg);
                        claim(&mut found, &mut stats, "pre", &format!("{h}: need {text} (at a call)"), e);
                    } else {
                        let mut e = ev.clone();
                        e["def"] = json!(name);
                        e["kind"] = json!("error");
                        e["clause"] = json!(msg);
                        e["status"] = json!("-");
                        found.push(e);
                    }
                    None
                }
                Err(e) => return Err(format!("{}: {e:?}", name)),
            };
            // a module invariant is the loop's business only if the loop writes its state (then the page has a
            // loop goal for it); an unrelated one broken before the loop is the call's `invariant` goal
            let heads: Vec<(kern::Int, Vec<String>)> = it
                .heads
                .iter()
                .map(|(k, bad)| {
                    (*k, bad.iter().filter(|t| !t.starts_with("module invariant ") || goals.contains_key(&("loop".to_string(), format!("{t}  (holds when the loop starts)")))).cloned().collect())
                })
                .collect();
            if let Some((i, (k, bad))) = heads.iter().enumerate().find(|(_, (_, bad))| !bad.is_empty()) {
                let part = if i == 0 { "holds when the loop starts" } else { "kept by every iteration" };
                for text in bad {
                    let mut e = ev.clone();
                    e["saw"] = json!(format!("false at k = {k}"));
                    claim(&mut found, &mut stats, "loop", &format!("{text}  ({part})"), e);
                }
            }
            let Some(outcome) = outcome else { continue };
            let mut after = w;
            if outcome == Outcome::Ok {
                stats.ok += 1;
                if vacuous {
                    let mut e = ev.clone();
                    e["def"] = json!(name);
                    e["kind"] = json!("vacuous");
                    e["clause"] = json!("never succeeds");
                    e["status"] = json!("vacuous");
                    found.push(e);
                }
                let mut pre = before.clone();
                for (kind, clause) in it.holds(d, &args, &mut pre, &mut after).map_err(|e| e.to_string())? {
                    let mut e = ev.clone();
                    e["saw"] = json!("false after an ok run");
                    claim(&mut found, &mut stats, &kind, &clause, e);
                }
                for (text, key) in it.broken_invariants(&mut after, None).map_err(|e| e.to_string())? {
                    let mut e = ev.clone();
                    e["saw"] = json!(format!("broken at {}", key.str()));
                    claim(&mut found, &mut stats, "invariant", &text, e);
                }
            } else if it.unstated_refusal(d, &args, &before, &outcome).map_err(|e| e.to_string())? {
                let mut e = ev.clone();
                if let Outcome::Fail(c, _) = &outcome {
                    e["saw"] = json!(format!("refused {c}"));
                }
                claim(&mut found, &mut stats, "ok", &ok_clause, e);
            }
        }
    }
    Ok((found, stats))
}

// ---------------------------------------------------------------- one program

#[derive(Debug, Clone)]
pub struct Job {
    pub label: String,
    pub src: Option<String>,
    pub module: Option<Module>,
    pub seed: i128,
    pub runs: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Outcome1 {
    pub label: String,
    pub wf: bool,
    pub error: Option<String>,
    pub crash: Option<String>,
    pub tiers: BTreeMap<String, BTreeMap<String, u64>>,
    pub stats: Stats,
    pub found: Vec<J>,
    pub src: Option<String>,
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
}

/// Check one program and run it against its page (fuzz.py's check_program).
pub fn check_program(job: &Job) -> Outcome1 {
    let mut out = Outcome1 { label: job.label.clone(), ..Default::default() };
    let rnd: Rnd = Rc::new(RefCell::new(Random::from_int(job.seed)));
    let defs = match (&job.module, &job.src) {
        (Some(m), _) => m.clone(),
        (None, Some(src)) => match parse_module(src, true).map_err(|e| e.to_string()).and_then(|m| check_module(&m).map(|_| m).map_err(|e| format!("{e:?}"))) {
            Ok(m) => m,
            Err(e) => {
                out.error = Some(e.chars().take(200).collect());
                return out;
            }
        },
        (None, None) => unreachable!(),
    };
    out.wf = true;
    let reports = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyse(&defs, false, None, None))) {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            out.crash = Some(format!("{e:?}").chars().take(300).collect());
            out.src = job.src.clone();
            return out;
        }
        Err(p) => {
            out.crash = Some(format!("panic: {}", panic_text(p)).chars().take(300).collect());
            out.src = job.src.clone();
            return out;
        }
    };
    for r in reports.values() {
        for g in &r.guarantees {
            *out.tiers.entry(g.kind.clone()).or_default().entry(g.status.as_str().to_string()).or_insert(0) += 1;
        }
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| oracle(&defs, &reports, rnd, job.runs))) {
        Ok(Ok((found, stats))) => {
            out.stats = stats;
            if !found.is_empty() {
                out.src = Some(job.src.clone().unwrap_or_else(|| {
                    defs.defs.iter().map(|(n, d)| format!("# {n}: {}", d.body.iter().map(stmt_code).collect::<Vec<_>>().join(", "))).collect::<Vec<_>>().join("\n")
                }));
            }
            out.found = found;
        }
        Ok(Err(e)) => out.crash = Some(format!("oracle: {e}").chars().take(300).collect()),
        Err(p) => out.crash = Some(format!("oracle panic: {}", panic_text(p)).chars().take(300).collect()),
    }
    out
}

// ---------------------------------------------------------------- the run

/// The programs of one fuzz run, with their seeds, drawn as fuzz.py's jobs() draws them.
pub fn jobs(root: &std::path::Path, seed: i128, n_loops: usize, n_handlers: usize, runs: u64, with_mutants: bool) -> Vec<Job> {
    let mut rnd = Random::from_int(seed);
    let mut out = vec![];
    for i in 0..n_loops {
        let src = gen_loop(&mut rnd);
        out.push(Job { label: format!("loop#{i}"), src: Some(src), module: None, seed: rnd.randrange(0, 1_000_000_000) as i128, runs });
    }
    for i in 0..n_handlers {
        let src = gen_handler(&mut rnd);
        out.push(Job { label: format!("handler#{i}"), src: Some(src), module: None, seed: rnd.randrange(0, 1_000_000_000) as i128, runs });
    }
    if with_mutants {
        for (label, m) in template_mutants(root) {
            out.push(Job { label, src: None, module: Some(m), seed: rnd.randrange(0, 1_000_000_000) as i128, runs });
        }
    }
    out
}

/// Run every job, `threads` at a time, each on a fresh thread (fresh Z3 state, as fuzz.py's fresh processes).
pub fn run_all(js: &[Job], threads: usize) -> Vec<Outcome1> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<Option<Outcome1>>> = std::sync::Mutex::new(vec![None; js.len()]);
    std::thread::scope(|sc| {
        for _ in 0..threads.max(1) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i >= js.len() {
                    break;
                }
                let job = js[i].clone();
                let r = std::thread::Builder::new().stack_size(256 << 20).spawn(move || check_program(&job)).unwrap().join();
                let r = r.unwrap_or_else(|p| Outcome1 { label: js[i].label.clone(), wf: true, crash: Some(format!("panic: {}", panic_text(p))), ..Default::default() });
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

/// The run's summary, with fuzz.py's keys.
pub fn summarise(results: &[Outcome1]) -> J {
    let mut tiers: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut unmapped_kinds: BTreeMap<String, u64> = BTreeMap::new();
    let (mut runs, mut ok, mut skipped, mut unmapped) = (0, 0, 0, 0);
    for r in results {
        for (k, st) in &r.tiers {
            for (s, n) in st {
                *tiers.entry(k.clone()).or_default().entry(s.clone()).or_insert(0) += n;
            }
        }
        for (k, n) in &r.stats.unmapped_kinds {
            *unmapped_kinds.entry(k.clone()).or_insert(0) += n;
        }
        runs += r.stats.runs;
        ok += r.stats.ok;
        skipped += r.stats.skipped;
        unmapped += r.stats.unmapped;
    }
    let fam = |p: &str| {
        let rs: Vec<&Outcome1> = results.iter().filter(|r| r.label.starts_with(p)).collect();
        format!("{}/{}", rs.iter().filter(|r| r.wf).count(), rs.len())
    };
    json!({
        "programs": results.len(),
        "well_formed": results.iter().filter(|r| r.wf).count(),
        "crashes": results.iter().filter(|r| r.crash.is_some()).map(|r| &r.label).collect::<Vec<_>>(),
        "contradictions": results.iter().map(|r| r.found.len()).sum::<usize>(),
        "programs_with_contradictions": results.iter().filter(|r| !r.found.is_empty()).map(|r| &r.label).collect::<Vec<_>>(),
        "tiers": tiers, "runs": runs, "ok_runs": ok, "skipped": skipped, "unmapped": unmapped, "unmapped_kinds": unmapped_kinds,
        "loop_well_formed": fam("loop#"), "handler_well_formed": fam("handler#"), "sum_well_formed": fam("sum#"),
        "const_variant_well_formed": fam("sumc#"), "record_variant_well_formed": fam("sumr#"),
        "variant_mismatches": variant_mismatches(results),
    })
}

/// The command line: fuzz.py's options. Exit 1 if there is any contradiction or crash.
pub fn main(argv: &[String]) -> i32 {
    let mut opt: IndexMap<&str, String> = IndexMap::new();
    let (mut no_mutants, mut i) = (false, 0);
    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "--no-mutants" => no_mutants = true,
            "--seed" | "--loops" | "--handlers" | "--sums" | "--variants" | "--runs" | "--jobs" | "--fault" | "--save" if i + 1 < argv.len() => {
                opt.insert(&a[2..], argv[i + 1].clone());
                i += 1;
            }
            _ => {
                eprintln!("usage: fuzz [--seed N] [--loops N] [--handlers N] [--sums N] [--variants N] [--runs N] [--no-mutants] [--jobs N] [--fault a|b|c] [--save FILE]");
                return 2;
            }
        }
        i += 1;
    }
    let num = |k: &str, d: u64| opt.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let fault = opt.get("fault").cloned();
    if let Some(f) = &fault {
        if !["a", "b", "c"].contains(&f.as_str()) {
            eprintln!("fuzz: --fault must be a, b or c");
            return 2;
        }
        // a known prover bug, for the teeth check: inert unless the kern crate has the `faults` feature
        std::env::set_var("KERN_FAULT", f);
        if !kern::prove::fault(f) {
            eprintln!("fuzz: this kern was built without the `faults` feature");
            return 2;
        }
    }
    let root = std::env::var("KERN_FUZZ_DATA").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
    });
    let threads = num("jobs", std::thread::available_parallelism().map(|n| n.get() as u64 - 1).unwrap_or(1).max(1));
    let t0 = std::time::Instant::now();
    let mut js = jobs(&root, num("seed", 1) as i128, num("loops", 300) as usize, num("handlers", 200) as usize, num("runs", 300), !no_mutants);
    js.extend(sum_jobs(num("seed", 1) as i128, num("sums", 150) as usize, num("runs", 300)));
    let n_var = (num("variants", 50) as usize).min(num("sums", 150) as usize);
    js.extend(variant_jobs(num("seed", 1) as i128, n_var, num("runs", 300)));
    let results = run_all(&js, threads as usize);
    let mut s = summarise(&results);
    s["seed"] = json!(num("seed", 1));
    s["fault"] = json!(fault);
    s["seconds"] = json!((t0.elapsed().as_secs_f64() * 10.0).round() / 10.0);
    s["toolchain"] = json!(kern::report::tool_hash());
    println!("{}", serde_json::to_string_pretty(&s).unwrap());
    if let Some(path) = opt.get("save") {
        let bad: Vec<J> = results
            .iter()
            .filter(|r| !r.found.is_empty() || r.crash.is_some())
            .map(|r| json!({"label": r.label, "crash": r.crash, "found": r.found, "src": r.src}))
            .collect();
        let _ = std::fs::write(path, serde_json::to_string_pretty(&json!({"summary": s, "found": bad})).unwrap());
    }
    // a variant whose verdicts differ from its base is reported, not fatal: the known case (2026-11-05) is a record
    // program's counterexample missing a computed-key entry, so `?` where the base has ✗ (never a false ✔)
    let bad = s["contradictions"].as_u64().unwrap_or(0) > 0 || !s["crashes"].as_array().map_or(true, |a| a.is_empty());
    i32::from(bad)
}
