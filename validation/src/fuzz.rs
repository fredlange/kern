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
