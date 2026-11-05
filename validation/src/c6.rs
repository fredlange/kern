//! `c6`: C6's frozen runner (experiments/challenge/c6/run.py, frozen at d3db476), on the Rust toolchain.
//!
//! Same plan, same generator (crate::chains, byte-identical to gen.py: `c6 --emit D K E N STATE` prints a
//! module for comparison), same measurement: the time of `analyse` with mutation testing, after parsing and
//! well-formedness, each configuration in its own child process. Not measured here: peak memory (run.py read
//! `ru_maxrss`; std has no getrusage) and run.py's re-prove check (the CLI's result cache replaced it, S2).

use std::time::Instant;

/// run.py's PLAN: (tag, d, k, e, n, state).
pub fn plan() -> Vec<(&'static str, [usize; 5])> {
    let mut p = vec![("P1", [6, 4, 6, 10, 0]), ("P2", [3, 2, 3, 200, 0])];
    p.extend((1..=8).map(|d| ("d", [d, 2, 3, d, 0])));
    p.extend((0..=6).map(|k| ("k", [3, k, 3, 3, 0])));
    p.extend((0..=8).map(|e| ("e", [3, 2, e, 3, 0])));
    p.extend([10, 50].map(|n| ("n", [3, 2, 3, n, 0])));
    p.extend([0, 1].map(|s| ("state", [3, 2, 3, 10, s])));
    p
}

/// One configuration, in this process: the JSON line run.py's child printed (without max_rss_mb).
pub fn measure_one(c: [usize; 5]) -> serde_json::Value {
    let [d, k, e, n, state] = c;
    let src = crate::chains::chains(d, k, e, n, state != 0);
    let defs = kern::syntax::parse_module(&src, true).expect("generated module parses");
    kern::wf::check_module(&defs).expect("generated module is well-formed");
    let t = Instant::now();
    let reps = kern::report::analyse(&defs, true, None, None).expect("analyse");
    let secs = t.elapsed().as_secs_f64();
    let muts: Vec<_> = reps.values().filter_map(|r| r.mutation.as_ref()).collect();
    serde_json::json!({
        "secs": secs, "per_def": secs / defs.defs.len() as f64, "defs": defs.defs.len(),
        "clean": reps.values().filter(|r| r.clean).count(),
        "guarantees": reps.values().map(|r| r.guarantees.len()).sum::<usize>(),
        "mutants": muts.iter().map(|m| m.total).sum::<u64>(),
    })
}

pub fn main(argv: &[String]) -> i32 {
    let nums = |a: &[String]| -> [usize; 5] {
        let v: Vec<usize> = a.iter().map(|x| x.parse().expect("number")).collect();
        [v[0], v[1], v[2], v[3], v[4]]
    };
    match argv.first().map(String::as_str) {
        Some("--emit") => {
            let [d, k, e, n, s] = nums(&argv[1..6]);
            print!("{}", crate::chains::chains(d, k, e, n, s != 0));
            0
        }
        Some("--one") => {
            println!("{}", measure_one(nums(&argv[1..6])));
            0
        }
        _ => {
            let exe = std::env::current_exe().expect("own path");
            for (tag, c) in plan() {
                let args: Vec<String> = c.iter().map(|x| x.to_string()).collect();
                let out = std::process::Command::new(&exe).arg("--one").args(&args).output().expect("child");
                let mut row = serde_json::json!({"tag": tag, "d": c[0], "k": c[1], "e": c[2], "n": c[3], "state": c[4]});
                if out.status.success() {
                    let r: serde_json::Value = serde_json::from_slice(&out.stdout).expect("child json");
                    row.as_object_mut().unwrap().extend(r.as_object().unwrap().clone());
                } else {
                    row["error"] = String::from_utf8_lossy(&out.stderr).chars().rev().take(800).collect::<String>().chars().rev().collect::<String>().into();
                }
                println!("{row}");
            }
            0
        }
    }
}
