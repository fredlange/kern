//! Refusal conditions lifted past direct writes (SHORTCOMINGS.md S7), checked against the interpreter.
//!
//! The prover assumes a callee's lifted conditions on the caller's success path (prove.rs, modular_call), so a wrong
//! rewrite through the writes (aliasing above all) would let it prove false claims. The soundness fuzzer never
//! generates a body `need` after a write, so it doesn't gate this. These programs do: `step` writes directly
//! (balances and a state map, often to keys that may alias), then refuses on a read of the written state; `outer`
//! calls it and claims things about its pre-state that hold only if `step`'s lifted condition is right. Every claim
//! the page proves is run against the interpreter (fuzz's oracle, which draws aliased accounts often).
use std::path::{Path, PathBuf};

use kern::pyrandom::Random;
use kern_validation::fuzz::{check_program, summarise, Job};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn pick<'a>(rnd: &mut Random, xs: &[&'a str]) -> &'a str {
    *rnd.choice(xs)
}

fn gen(rnd: &mut Random) -> String {
    let mut step = vec!["step a:Account b:Account n:Int".to_string()];
    if rnd.random() < 0.4 {
        step.push("  need a != b else Same".into());
    }
    if rnd.random() < 0.5 {
        step.push("  need n >= 0 else Neg".into());
    }
    step.push("body".into());
    let writes = [
        "ledger.debit a n",
        "ledger.credit b n",
        "ledger.debit b 1",
        "ledger.credit a 2",
        "hold.set 0 n",
        "hold.set n 3",
        "hold.set 1 (hold.get 0 + 1)",
    ];
    let k = *rnd.choice(&[1usize, 2, 2, 3]);
    for w in rnd.sample(&writes, k) {
        step.push(format!("  {w}"));
    }
    let reads = ["ledger.balance a", "ledger.balance b", "ledger.balance a + ledger.balance b", "ledger.total", "hold.get 0", "hold.get 1", "hold.get n"];
    let mut needs: Vec<(&str, &str, i64)> = vec![];
    let mut need = |rnd: &mut Random, ctor: &str| {
        let (r, op, k) = (pick(rnd, &reads), pick(rnd, &[">=", "<=", "<"]), *rnd.choice(&[0i64, 5, 10, 30]));
        needs.push((r, op, k));
        format!("need {r} {op} {k} else {ctor}")
    };
    if rnd.random() < 0.3 {
        step.push(format!("  if n {} 3", pick(rnd, &[">", "<="])));
        let n = need(rnd, "Big");
        step.push(format!("    {n}"));
    } else {
        let n = need(rnd, "Big");
        step.push(format!("  {n}"));
    }
    if rnd.random() < 0.3 {
        let n = need(rnd, "Big2");
        step.push(format!("  {n}"));
    }
    let args = pick(rnd, &["x y m", "x x m", "y x 3", "x z m", "x y (m + 1)"]);
    let actual: Vec<&str> = args.split(' ').collect::<Vec<_>>();
    let (aa, ab, an) = (actual[0], actual[1], if args.ends_with("(m + 1)") { "(m + 1)" } else { actual[2] });
    // candidate claims: each need of `step` in outer's terms, read before the writes, with the deltas the writes
    // might have added (some right, most wrong), and a looser bound
    let mut claims: Vec<String> = vec![];
    for (r, op, k) in &needs {
        let r = r.replace("ledger.balance a", &format!("ledger.balance {aa}")).replace("ledger.balance b", &format!("ledger.balance {ab}")).replace("hold.get n", &format!("hold.get {an}"));
        for d in ["", " + N", " - N", " - 1", " + 2", " - N + 2", " + N - 1", " - N - 1"] {
            let d = d.replace('N', an);
            let looser = if *op == ">=" { format!("{k} - 20") } else { format!("{k} + 20") };
            claims.push(format!("{r}{d} {op} {k}"));
            claims.push(format!("{r}{d} {op} {looser}"));
        }
    }
    claims.extend(
        ["ledger.balance x + m <= 10", "ledger.balance y >= 5", "ledger.total <= 30", "m >= 5", "hold.get 0 <= 10"].iter().map(|c| c.to_string()),
    );
    let k = *rnd.choice(&[2usize, 3, 4]);
    let mut outer = vec!["outer x:Account y:Account z:Account m:Int".to_string()];
    for c in rnd.sample(&claims, k) {
        outer.push(format!("  ensure {c}"));
    }
    outer.extend(["body".into(), format!("  step {args}")]);
    format!("state hold Int\n\n{}\n\n{}\n", step.join("\n"), outer.join("\n"))
}

#[test]
fn lifted_conditions_past_writes_agree_with_the_interpreter() {
    let _ = root();
    let mut rnd = Random::from_int(1);
    let jobs: Vec<Job> = (0..160)
        .map(|i| {
            let src = gen(&mut rnd);
            Job { label: format!("lifted#{i}"), src: Some(src), module: None, seed: rnd.randrange(0, 1_000_000_000) as i128, runs: 200 }
        })
        .collect();
    let results: Vec<_> = jobs.iter().map(check_program).collect();
    let s = summarise(&results);
    assert_eq!(s["contradictions"], 0, "{}", s["programs_with_contradictions"]);
    assert!(s["crashes"].as_array().unwrap().is_empty(), "{}", s["crashes"]);
    assert_eq!(s["well_formed"], s["programs"], "every generated program is well-formed");
    // not vacuous: the callers' claims come out both ways, and many runs succeed
    let ens = &s["tiers"]["ensure"];
    let (proven, refuted) = (ens["proven"].as_u64().unwrap_or(0), ens["refuted"].as_u64().unwrap_or(0));
    eprintln!("ensure tiers: {ens}");
    assert!(proven >= 40 && refuted >= 40, "{ens}");
}

