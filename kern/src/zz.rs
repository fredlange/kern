//! Z3 terms built exactly as z3py builds them, so the Rust prover hands the solver the same input as kern/prove.py
//! and gets the same answers (docs/research/RUST_PORT_PLAN.md). Where the `z3` crate already makes the same C API
//! call as z3py it is used directly. Where it doesn't, z3-sys is called here:
//! - quantifiers: z3py's ForAll/Exists use Z3_mk_quantifier_const_ex with weight 1 and empty qid/skid symbols;
//!   the crate's forall_const uses Z3_mk_forall_const with weight 0;
//! - `!=`: z3py builds Z3_mk_distinct of two arguments, not not(eq);
//! - numerals: z3py's IntVal is Z3_mk_numeral of the decimal string (any size).
//! Every solver gets the step limit `rlimit`, never a wall-clock timeout (L1).

use std::ffi::CString;

use z3::ast::{Array, Ast, Bool, Dynamic, Int as ZInt};
use z3::{Context, FuncDecl, Model, Optimize, Params, RecFuncDecl, SatResult, Solver, Sort};

/// Python's `RLIMIT` (kern/prove.py): Z3 steps per check.
pub const RLIMIT: u32 = 10_000_000;

fn ctx() -> Context {
    Context::thread_local()
}

/// z3.Int(name)
pub fn int(name: &str) -> ZInt {
    ZInt::new_const(name)
}

/// z3.IntVal(n): Z3_mk_numeral of the decimal string, as z3py does.
pub fn val(n: i128) -> ZInt {
    let c = CString::new(n.to_string()).unwrap();
    unsafe {
        let s = Sort::int();
        let a = z3_sys::Z3_mk_numeral(ctx().get_z3_context(), c.as_ptr(), s.get_z3_sort()).expect("Z3 numeral");
        ZInt::wrap(&ctx(), a)
    }
}

/// z3.BoolVal(b)
pub fn bval(b: bool) -> Bool {
    Bool::from_bool(b)
}

/// z3.Array(name, IntSort(), IntSort())
pub fn array(name: &str) -> Array {
    Array::new_const(name, &Sort::int(), &Sort::int())
}

pub fn select(a: &Array, i: &ZInt) -> ZInt {
    a.select(i).as_int().expect("an Int -> Int array")
}

pub fn store(a: &Array, i: &ZInt, v: &ZInt) -> Array {
    a.store(i, v)
}

pub fn add(a: &ZInt, b: &ZInt) -> ZInt {
    ZInt::add(&[a, b])
}

pub fn sub(a: &ZInt, b: &ZInt) -> ZInt {
    ZInt::sub(&[a, b])
}

pub fn mul(a: &ZInt, b: &ZInt) -> ZInt {
    ZInt::mul(&[a, b])
}

/// z3py `a / b` on Ints: Z3_mk_div.
pub fn div(a: &ZInt, b: &ZInt) -> ZInt {
    a.div(b)
}

/// Is this term a numeral? In z3py it is then an `IntNumRef`, a subclass of `ArithRef`, and Python's operator
/// rule (the right operand's reflected method goes first when its type is a subclass of the left's) makes
/// `x > IntVal(0)` build `(< 0 x)`. Comparisons and `==`/`!=` follow that rule below; `+ - * /` don't swap.
pub fn is_num(x: &ZInt) -> bool {
    unsafe { z3_sys::Z3_is_numeral_ast(ctx().get_z3_context(), x.get_z3_ast()) }
}

fn swapped(a: &ZInt, b: &ZInt) -> bool {
    is_num(b) && !is_num(a)
}

/// z3py `a == b` for two Int terms.
pub fn eq_int(a: &ZInt, b: &ZInt) -> Bool {
    if swapped(a, b) { b.eq(a) } else { a.eq(b) }
}

pub fn eq_bool(a: &Bool, b: &Bool) -> Bool {
    a.eq(b)
}

pub fn eq_array(a: &Array, b: &Array) -> Bool {
    a.eq(b)
}

/// z3py `a != b`: Z3_mk_distinct(2, [a, b]) (operands swapped as for `==`).
pub fn ne_int(a: &ZInt, b: &ZInt) -> Bool {
    if swapped(a, b) { ZInt::distinct(&[b, a]) } else { ZInt::distinct(&[a, b]) }
}

pub fn ne_bool(a: &Bool, b: &Bool) -> Bool {
    Bool::distinct(&[a, b])
}

/// z3py `a >= b` for two Int terms: with a numeral on the right only, `b <= a`.
pub fn ge(a: &ZInt, b: &ZInt) -> Bool {
    if swapped(a, b) { b.le(a) } else { a.ge(b) }
}

pub fn le(a: &ZInt, b: &ZInt) -> Bool {
    if swapped(a, b) { b.ge(a) } else { a.le(b) }
}

pub fn gt(a: &ZInt, b: &ZInt) -> Bool {
    if swapped(a, b) { b.lt(a) } else { a.gt(b) }
}

pub fn lt(a: &ZInt, b: &ZInt) -> Bool {
    if swapped(a, b) { b.gt(a) } else { a.lt(b) }
}

/// z3py `a >= n` with a Python int n: never swapped (an int is no ArithRef subclass).
pub fn ge_n(a: &ZInt, n: i128) -> Bool {
    a.ge(&val(n))
}

pub fn le_n(a: &ZInt, n: i128) -> Bool {
    a.le(&val(n))
}

/// z3py `a == n` with a Python int n: never swapped.
pub fn eq_n(a: &ZInt, n: i128) -> Bool {
    a.eq(&val(n))
}

/// z3.And(*xs): Z3_mk_and with every argument, none dropped (z3py doesn't simplify).
pub fn and(xs: &[Bool]) -> Bool {
    let refs: Vec<&Bool> = xs.iter().collect();
    Bool::and(&refs)
}

pub fn or(xs: &[Bool]) -> Bool {
    let refs: Vec<&Bool> = xs.iter().collect();
    Bool::or(&refs)
}

pub fn not(a: &Bool) -> Bool {
    a.not()
}

pub fn implies(a: &Bool, b: &Bool) -> Bool {
    a.implies(b)
}

/// z3.If(c, a, b) on Ints
pub fn ite(c: &Bool, a: &ZInt, b: &ZInt) -> ZInt {
    c.ite(a, b)
}

/// z3.Distinct(*xs)
pub fn distinct(xs: &[ZInt]) -> Bool {
    let refs: Vec<&ZInt> = xs.iter().collect();
    ZInt::distinct(&refs)
}

// z3-sys types Z3_symbol as NonNull, but z3py's empty qid/skid ("") is Z3's null symbol (Z3_mk_string_symbol("")
// returns null). So this one function is declared here with raw pointers; it links against the same libz3.
extern "C" {
    #[link_name = "Z3_mk_quantifier_const_ex"]
    fn mk_quantifier_const_ex(
        c: *mut std::ffi::c_void,
        is_forall: bool,
        weight: std::ffi::c_uint,
        quantifier_id: *mut std::ffi::c_void,
        skolem_id: *mut std::ffi::c_void,
        num_bound: std::ffi::c_uint,
        bound: *const *mut std::ffi::c_void,
        num_patterns: std::ffi::c_uint,
        patterns: *const *mut std::ffi::c_void,
        num_no_patterns: std::ffi::c_uint,
        no_patterns: *const *mut std::ffi::c_void,
        body: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
}

fn quantifier(forall: bool, vars: &[&ZInt], body: &Bool) -> Bool {
    let c = ctx().get_z3_context();
    let apps: Vec<*mut std::ffi::c_void> = vars
        .iter()
        .map(|v| unsafe { z3_sys::Z3_to_app(c, v.get_z3_ast()).expect("a bound variable").as_ptr() as *mut _ })
        .collect();
    unsafe {
        let q = mk_quantifier_const_ex(
            c.as_ptr() as *mut _,
            forall,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            apps.len() as u32,
            apps.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            body.get_z3_ast().as_ptr() as *mut _,
        );
        let q = std::ptr::NonNull::new(q as *mut z3_sys::_Z3_ast).expect("Z3 quantifier");
        Bool::wrap(&ctx(), q)
    }
}

/// z3.ForAll(vars, body): weight 1, empty qid and skid, no patterns.
pub fn forall(vars: &[&ZInt], body: &Bool) -> Bool {
    quantifier(true, vars, body)
}

/// z3.Exists(vars, body)
pub fn exists(vars: &[&ZInt], body: &Bool) -> Bool {
    quantifier(false, vars, body)
}

/// z3.Function(name, IntSort() * (n + 1)) applied to args
pub fn apply_fn(name: &str, args: &[ZInt]) -> ZInt {
    let sorts: Vec<Sort> = args.iter().map(|_| Sort::int()).collect();
    let srefs: Vec<&Sort> = sorts.iter().collect();
    let f = FuncDecl::new(name, &srefs, &Sort::int());
    let a: Vec<&dyn Ast> = args.iter().map(|x| x as &dyn Ast).collect();
    f.apply(&a).as_int().expect("an Int function")
}

/// An uninterpreted Int function of arguments of any sort (a function that reads state takes its arrays too).
pub fn apply_fn_dyn(name: &str, args: &[Dynamic]) -> ZInt {
    let sorts: Vec<Sort> = args.iter().map(|x| x.get_sort()).collect();
    let srefs: Vec<&Sort> = sorts.iter().collect();
    let f = FuncDecl::new(name, &srefs, &Sort::int());
    let a: Vec<&dyn Ast> = args.iter().map(|x| x as &dyn Ast).collect();
    f.apply(&a).as_int().expect("an Int function")
}

/// A sum as a recursive function, defined from the right so a loop step is one unfolding:
/// `f(xs…, lo, hi) = if hi <= lo then 0 else f(xs…, lo, hi - 1) + step`, where `step` is a term over the formal
/// constants `xs`, `lo` and `hi` (built with the summed variable = hi - 1). Z3 unfolds it on demand.
pub fn rec_sum(name: &str, formals: &[Dynamic], lo: &ZInt, hi: &ZInt, step: &ZInt) -> RecFuncDecl {
    let mut sorts: Vec<Sort> = formals.iter().map(|f| f.get_sort()).collect();
    sorts.push(Sort::int());
    sorts.push(Sort::int());
    let srefs: Vec<&Sort> = sorts.iter().collect();
    let f = RecFuncDecl::new(name, &srefs, &Sort::int());
    let hi1 = sub(hi, &val(1));
    let mut rec_args: Vec<&dyn Ast> = formals.iter().map(|x| x as &dyn Ast).collect();
    rec_args.push(lo);
    rec_args.push(&hi1);
    let rec = f.apply(&rec_args).as_int().expect("an Int sum");
    let body = ite(&le(hi, lo), &val(0), &add(&rec, step));
    let mut args: Vec<&dyn Ast> = formals.iter().map(|x| x as &dyn Ast).collect();
    args.push(lo);
    args.push(hi);
    f.add_def(&args, &body);
    f
}

/// A sum function applied to actual arguments (the formals' actuals, then lo and hi).
pub fn apply_rec(f: &RecFuncDecl, args: &[Dynamic]) -> ZInt {
    let a: Vec<&dyn Ast> = args.iter().map(|x| x as &dyn Ast).collect();
    f.apply(&a).as_int().expect("an Int sum")
}

fn limited() -> Params {
    let mut p = Params::new();
    p.set_u32("rlimit", RLIMIT);
    p
}

/// z3.Solver() with s.set("rlimit", RLIMIT)
pub fn solver() -> Solver {
    let s = Solver::new();
    s.set_params(&limited());
    s
}

/// z3.Optimize() with o.set("rlimit", RLIMIT)
pub fn optimize() -> Optimize {
    let o = Optimize::new();
    o.set_params(&limited());
    o
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Sat,
    Unsat,
    Unknown,
}

pub fn check(s: &Solver) -> Check {
    if crate::prove::fault("unknown") {
        return Check::Unknown; // the prover gets no answer from any solver (a test of the ◐ and "no answer" tiers)
    }
    if std::env::var_os("KERN_Z3_TRACE").is_some() {
        eprintln!("=== check\n{s}");
    }
    match s.check() {
        SatResult::Sat => Check::Sat,
        SatResult::Unsat => Check::Unsat,
        SatResult::Unknown => Check::Unknown,
    }
}

pub fn check_opt(o: &Optimize) -> Check {
    if std::env::var_os("KERN_Z3_TRACE").is_some() {
        eprintln!("=== check\n{o}");
    }
    match o.check(&[]) {
        SatResult::Sat => Check::Sat,
        SatResult::Unsat => Check::Unsat,
        SatResult::Unknown => Check::Unknown,
    }
}

/// `m.eval(x, model_completion=True).as_long()`: None when the value isn't a numeral (z3py would raise).
pub fn eval_long(m: &Model, x: &ZInt) -> Option<i128> {
    numeral(&m.eval(x, true)?)
}

/// A numeral's exact value (any size), via Z3's decimal string.
pub fn numeral(x: &ZInt) -> Option<i128> {
    unsafe {
        let c = ctx().get_z3_context();
        if !z3_sys::Z3_is_numeral_ast(c, x.get_z3_ast()) {
            return None;
        }
        let s = z3_sys::Z3_get_numeral_string(c, x.get_z3_ast());
        if s.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(s).to_str().ok()?.parse().ok()
    }
}

/// `_array_entries`: (key, value) pairs a model's array value spells out (Store chains, then an as-array
/// function's entry table), at most `limit`; stops quietly at anything that isn't a numeral, as Python does.
pub fn array_entries(m: &Model, v: &Array, limit: usize) -> Vec<(i128, i128)> {
    let mut out = vec![];
    let mut v = v.clone();
    loop {
        if out.len() >= limit {
            break;
        }
        let Ok(d) = v.safe_decl() else { break };
        if d.kind() != z3::DeclKind::Store {
            break;
        }
        let ch = v.children();
        let (Some(k), Some(x)) = (ch[1].as_int().and_then(|i| numeral(&i)), ch[2].as_int().and_then(|i| numeral(&i)))
        else {
            return out;
        };
        out.push((k, x));
        v = match ch[0].as_array() {
            Some(a) => a,
            None => return out,
        };
    }
    if v.is_as_array() {
        let Some(f) = v.as_array_func_decl() else { return out };
        let Some(fi) = m.get_func_interp(&f) else { return out };
        for e in fi.get_entries().into_iter().take(limit) {
            let args = e.get_args();
            let (Some(k), Some(x)) = (args[0].as_int().and_then(|i| numeral(&i)), e.get_value().as_int().and_then(|i| numeral(&i)))
            else {
                return out;
            };
            out.push((k, x));
        }
    }
    out
}

/// The s-expression of an assertion (find_cex checks for "forall").
pub fn sexpr(b: &Bool) -> String {
    b.to_string()
}

pub fn dynamic_of(b: &Bool) -> Dynamic {
    Dynamic::from_ast(b)
}
