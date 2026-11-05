//! Shared by the oracle tests: JSON (Python's AST tuples as arrays) -> AST.
#![allow(dead_code)]

use kern::syntax::{op, Emit, Expr, Stmt};
use serde_json::Value as J;

pub fn s(j: &J) -> String {
    j.as_str().unwrap().to_string()
}

pub fn expr(j: &J) -> Expr {
    let a = j.as_array().unwrap();
    match a[0].as_str().unwrap() {
        "int" => Expr::Int(a[1].as_i64().unwrap() as i128),
        "name" => Expr::Name(s(&a[1]), a[2].as_bool().unwrap()),
        "bin" => Expr::Bin(op(a[1].as_str().unwrap()), Box::new(expr(&a[2])), Box::new(expr(&a[3]))),
        "app" => Expr::App(s(&a[1][0]), a[1][1].as_bool().unwrap(), a[2].as_array().unwrap().iter().map(expr).collect()),
        "all" => Expr::All(s(&a[1]), Box::new(expr(&a[2])), Box::new(expr(&a[3])), Box::new(expr(&a[4]))),
        k => panic!("expr kind {k}"),
    }
}

pub fn exprs(j: &J) -> Vec<Expr> {
    j.as_array().unwrap().iter().map(expr).collect()
}

pub fn stmt(j: &J) -> Stmt {
    let a = j.as_array().unwrap();
    match a[0].as_str().unwrap() {
        "let" => Stmt::Let(s(&a[1]), expr(&a[2])),
        "need" => Stmt::Need(expr(&a[1]), s(&a[2]), exprs(&a[3])),
        "do" => Stmt::Do(expr(&a[1])),
        "if" => Stmt::If(expr(&a[1]), stmts(&a[2]), stmts(&a[3])),
        "for" => Stmt::For(s(&a[1]), expr(&a[2]), expr(&a[3]), exprs(&a[4]), stmts(&a[5])),
        "inv" => Stmt::Inv(expr(&a[1])),
        k => panic!("stmt kind {k}"),
    }
}

pub fn stmts(j: &J) -> Vec<Stmt> {
    j.as_array().unwrap().iter().map(stmt).collect()
}

pub fn emit(j: &J) -> Emit {
    let a = j.as_array().unwrap();
    Emit::Ev(s(&a[0]), exprs(&a[1]), a.get(2).filter(|c| !c.is_null()).map(expr))
}

pub const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

/// A fresh, empty scratch directory for one test (pytest's tmp_path).
pub fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("kern-test-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Copy a directory tree (skipping .kern-cache and proof-pages).
pub fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let name = e.file_name();
        let n = name.to_string_lossy();
        if n == ".kern-cache" || n == "proof-pages" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(&name));
        } else {
            std::fs::copy(&p, to.join(&name)).unwrap();
        }
    }
}

/// A copy of examples/<app> under a scratch directory.
pub fn copy_example(name: &str, app: &str) -> std::path::PathBuf {
    let dst = tmp(name).join(app);
    copy_tree(&std::path::Path::new(ROOT).join("examples").join(app), &dst);
    dst
}

/// Write `files` (relative path -> text) under `root`, with a kern.pkg when `pkg`.
pub fn write_tree(root: &std::path::Path, files: &[(&str, &str)], pkg: bool) -> std::path::PathBuf {
    std::fs::create_dir_all(root).unwrap();
    if pkg {
        std::fs::write(root.join("kern.pkg"), "").unwrap();
    }
    for (rel, src) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    root.to_path_buf()
}

/// Run the kern binary in `cwd`: (exit code, stdout, stderr).
pub fn kern(args: &[&str], cwd: &str) -> (i32, String, String) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_kern")).args(args).current_dir(cwd).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into())
}

/// Synthetic chain modules (port of validation/chains.py, itself a copy of C6's frozen generator): n definitions in
/// chains of depth d; f{c}_1 is a transfer-like leaf, f{c}_j calls f{c}_{j-1}. Every body has k sequential
/// `if amount > T` blocks, every definition e own emits; with `state`, every level increments `cnt.get a`.
pub fn chains(d: usize, k: usize, e: usize, n: usize, state: bool) -> String {
    let mut out: Vec<String> = vec![];
    if state {
        out.extend(["state cnt Account".to_string(), "invariant a:Account cnt.get a >= 0".into(), String::new()]);
    }
    let mut chains: Vec<Vec<usize>> = vec![(1..=d).collect(); n / d];
    if n % d != 0 {
        chains.push((1..=n % d).collect());
    }
    for (c, levels) in chains.iter().enumerate() {
        for &j in levels {
            out.extend(chain_def(c, j, k, e, state));
            out.push(String::new());
        }
    }
    out.join("\n")
}

fn chain_def(c: usize, j: usize, k: usize, e: usize, state: bool) -> Vec<String> {
    let (name, callee) = (format!("f{c}_{j}"), format!("f{c}_{}", j as i64 - 1));
    let cond = k.min(e);
    let mut lines = vec![format!("{name} a:Account b:Account amount:Int")];
    if j == 1 {
        lines.extend([
            "  need amount > 0 else BadAmount".to_string(),
            "  need a != b else SameAccount".into(),
            "  need ledger.balance a >= amount else Insufficient (amount - ledger.balance a)".into(),
        ]);
    } else {
        lines.push(format!("  need as {callee} a b amount"));
    }
    lines.extend([
        "  ok otherwise".to_string(),
        "  keep ledger.total".into(),
        "  ensure ledger.balance' a = ledger.balance a - amount".into(),
        "  ensure ledger.balance' b = ledger.balance b + amount".into(),
    ]);
    if state {
        lines.push(format!("  ensure cnt.get' a = cnt.get a + {j}"));
    }
    lines.extend((0..cond).map(|i| format!("  emit E{i} a amount clock.now when amount > {}", 10 * (i + 1))));
    if j > 1 {
        lines.push(format!("  emit as {callee} a b amount"));
    }
    lines.extend((cond..e).map(|i| format!("  emit F{i} a clock.now")));
    lines.push(format!("  eg A:500@u1 B:0@u2 | {name} A B 100 -> ok | A:400 B:100"));
    lines.push("body".into());
    if state {
        lines.push("  cnt.set a (cnt.get a + 1)".into());
    }
    for i in 0..k {
        lines.push(format!("  if amount > {}", 10 * (i + 1)));
        lines.push(if i < e { format!("    audit.emit (E{i} a amount clock.now)") } else { "    ledger.credit b 0".into() });
    }
    if j == 1 {
        lines.extend(["  ledger.debit a amount".to_string(), "  ledger.credit b amount".into()]);
    } else {
        lines.push(format!("  {callee} a b amount"));
    }
    lines.extend((cond..e).map(|i| format!("  audit.emit (F{i} a clock.now)")));
    lines
}
