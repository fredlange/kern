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
