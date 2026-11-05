//! Does `difftest` notice a wrong runtime? (port of validation/sensitivity.py.) Plant one known bug at a time in
//! kern-rt or in the emitter, run the differential test, and expect it to fail.
//!
//! ```text
//! sensitivity [--per-def N] [--dir DIR]
//! ```
//!
//! Python planted bugs in the checkout and restored them. Here the emitter is compiled into the tool, so every plant
//! needs a rebuild. To keep the checkout safe from a half-finished run, and from anyone building in it at the same
//! time, the plants go into a copy of the workspace (`DIR`, default `$TMPDIR/kern-sensitivity`), with its own target
//! and build directories. The copy is refreshed on every run and keeps its target, so later runs rebuild little.
//!
//! A planted bug the test doesn't catch is reported as MISSED: a hole in the test's coverage, not a pass. A plant
//! whose text no longer matches the source, or that stops the toolchain compiling, is reported too and counts as
//! missed, since then nothing was tested.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::difftest::ROOT;

const LIB: &str = "runtime/src/lib.rs";
const GEN: &str = "toolchain/src/emit_rust.rs";

/// (what, file, text, planted text)
pub const PLANTS: [(&str, &str, &str, &str); 13] = [
    ("refusal does not tick the clock", LIB, "if !matches!(out, Err(Stop::Fault(_))) {", "if out.is_ok() {"),
    ("refusal keeps its events", LIB, "            self.events.clear();\n        }\n        if !matches", "        }\n        if !matches"),
    ("division truncates toward zero", LIB, "a.div_euclid(b)", "a / b"),
    ("Int-map rollback keeps a new key", LIB, "w.imaps[m as usize].remove(&k);", "let _ = k;"),
    ("Account-map rollback skipped", LIB, "Undo::A(m, i, v) => w.amaps[m as usize][i as usize] = v,", "Undo::A(m, i, v) => { let _ = (m, i, v); }"),
    ("balance rollback skipped", LIB, "Undo::Bal(i, v) => w.bal[i as usize] = v,", "Undo::Bal(i, v) => { let _ = (i, v); }"),
    ("debit adds", LIB, "let v = sub(self.balance(a), n)?;", "let v = add(self.balance(a), n)?;"),
    ("total skips the last account", LIB, "self.w.bal.iter().try_fold", "self.w.bal[..self.w.bal.len().saturating_sub(1)].iter().try_fold"),
    ("`>=` compiled as `>`", GEN, "op => (format!(\"({} {} {})\", l, op, r), \"Bool\".into()),",
     "op => (format!(\"({} {} {})\", l, if op == \">=\" { \">\" } else { op }, r), \"Bool\".into()),"),
    ("arguments hoisted right to left", GEN, "format!(\"{{ {} {} }}\", lets.join(\" \"), fill(fmt, &temps))",
     "format!(\"{{ {} {} }}\", lets.iter().rev().cloned().collect::<Vec<_>>().join(\" \"), fill(fmt, &temps))"),
    ("refusal payload uses the first argument twice", GEN,
     "format!(\"({})\", vals.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join(\", \"))",
     "format!(\"({})\", vals.iter().map(|_| vals[0].0.as_str()).collect::<Vec<_>>().join(\", \"))"),
    ("channel events emitted on audit", GEN, "Self::hoist(&format!(\"tx.emit({}, {{}})\", i), &vals)",
     "Self::hoist(&format!(\"tx.emit({}, {{}})\", 0 * i), &vals)"),
    ("else branch dropped", GEN, "if !o.is_empty() {\n                        out.push(format!(\"{}}} else {{\", ind));",
     "if false && !o.is_empty() {\n                        out.push(format!(\"{}}} else {{\", ind));"),
];

/// What a sensitivity run copies: enough of the workspace to build and run difftest.
const COPY: [&str; 7] = ["Cargo.toml", "Cargo.lock", "toolchain", "runtime", "validation", "examples", "tests/probes"];

/// Copy `from` to `to`, writing only files whose content differs (cargo goes by mtime). Skips build output.
fn sync(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let name = e.file_name();
            if ["target", "build", "node_modules", ".kern-cache"].iter().any(|s| name == *s) {
                continue;
            }
            sync(&e.path(), &to.join(&name))?;
        }
        Ok(())
    } else {
        let new = std::fs::read(from)?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::read(to).ok().as_deref() != Some(&new[..]) {
            std::fs::write(to, new)?;
        }
        Ok(())
    }
}

/// difftest in the copy: (exit code, the TOTAL line or the tail of the output, the toolchain failed to compile).
fn difftest(dir: &Path, per_def: usize) -> (i32, String, bool) {
    let out = Command::new("cargo")
        .args(["run", "--release", "-q", "-p", "kern-validation", "--bin", "difftest", "--", "--per-def", &per_def.to_string()])
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env_remove("KERN_BUILD")
        .env_remove("KERN_RUNTIME")
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => return (-1, format!("cargo: {e}"), true),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let code = out.status.code().unwrap_or(-1);
    match stdout.lines().filter(|l| l.starts_with("TOTAL")).last() {
        Some(t) => (code, t.to_string(), false),
        None => {
            let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            (code, tail.chars().take(160).collect(), true)
        }
    }
}

pub fn main(argv: &[String]) -> i32 {
    let (mut per_def, mut dir) = (100usize, std::env::temp_dir().join("kern-sensitivity"));
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--per-def" => {
                i += 1;
                per_def = argv.get(i).and_then(|v| v.parse().ok()).expect("--per-def needs a number");
            }
            "--dir" => {
                i += 1;
                dir = PathBuf::from(argv.get(i).expect("--dir needs a path"));
            }
            other => {
                eprintln!("sensitivity: unknown argument {other}");
                return 2;
            }
        }
        i += 1;
    }
    for p in COPY {
        if let Err(e) = sync(&Path::new(ROOT).join(p), &dir.join(p)) {
            eprintln!("sensitivity: copying {p} to {}: {e}", dir.display());
            return 1;
        }
    }
    println!("workspace copy: {}", dir.display());
    let (code, total, broken) = difftest(&dir, per_def);
    println!("{:48} {}  {}", "clean", if code == 0 { "PASS" } else { "FAIL" }, total);
    if code != 0 || broken {
        return 1;
    }
    let mut missed = 0;
    for (what, file, old, new) in PLANTS {
        let path = dir.join(file);
        let src = std::fs::read_to_string(&path).unwrap();
        let n = src.matches(old).count();
        if n != 1 {
            println!("{what:48} PLANT DID NOT APPLY ({n} matches)");
            missed += 1;
            continue;
        }
        std::fs::write(&path, src.replacen(old, new, 1)).unwrap();
        let (code, total, broken) = difftest(&dir, per_def);
        std::fs::write(&path, &src).unwrap();
        let verdict = if broken {
            "PLANT BREAKS THE BUILD"
        } else if code != 0 {
            "caught"
        } else {
            "MISSED"
        };
        missed += (verdict != "caught") as usize;
        println!("{what:48} {verdict:14} {total}");
    }
    println!("missed {missed} of {}", PLANTS.len());
    (missed > 0) as i32
}
