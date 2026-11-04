//! zz.rs builds the same Z3 terms as z3py: each term below, printed as an s-expression, must equal what z3py prints
//! for the same construction (golden strings from the venv's z3-solver 5.3.3; regenerate with the snippet in the
//! commit that added this file if the constructions change).
use kern::zz::*;
use z3::ast::Ast;

const GOLDEN: &str = r#"
(forall ((j_3 Int)) (=> (and (<= lo j_3) (< j_3 hi)) (>= (select bal j_3) 0)))
(distinct x y)
(+ (select bal x) (* (- 1) y))
(+ 1208925819614629174706176 x)
and
(and (> x y))
(and true (> x y) (= x y))
(+ (ite (> x y) 1 0) 0)
(fn_fee x y)
(distinct x y lo)
(- (div x 3) (* y 2))
(= (store bal x (+ (select bal x) (* 1 y))) bal)
(exists ((j_3 Int)) (= (select bal j_3) 3))
(or (not (> x y)) (<= x (- 5)))
"#;

fn one_line(s: String) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn terms_print_like_z3py() {
    let (x, y, lo, hi, j) = (int("x"), int("y"), int("lo"), int("hi"), int("j_3"));
    let a = array("bal");
    let c = gt(&x, &y);
    let terms: Vec<String> = vec![
        forall(&[&j], &implies(&and(&[le(&lo, &j), lt(&j, &hi)]), &ge_n(&select(&a, &j), 0))).to_string(),
        ne_int(&x, &y).to_string(),
        add(&select(&a, &x), &mul(&val(-1), &y)).to_string(),
        add(&val(1 << 80), &x).to_string(),
        and(&[]).to_string(),
        and(&[c.clone()]).to_string(),
        and(&[bval(true), c.clone(), eq_int(&x, &y)]).to_string(),
        add(&ite(&c, &val(1), &val(0)), &val(0)).to_string(),
        apply_fn("fn_fee", &[x.clone(), y.clone()]).to_string(),
        distinct(&[x.clone(), y.clone(), lo.clone()]).to_string(),
        sub(&div(&x, &val(3)), &mul(&y, &val(2))).to_string(),
        eq_array(&store(&a, &x, &add(&select(&a, &x), &mul(&val(1), &y))), &a).to_string(),
        exists(&[&j], &eq_n(&select(&a, &j), 3)).to_string(),
        or(&[not(&c), le_n(&x, -5)]).to_string(),
    ];
    let want: Vec<&str> = GOLDEN.trim().lines().collect();
    assert_eq!(terms.len(), want.len());
    for (got, want) in terms.into_iter().zip(want) {
        assert_eq!(one_line(got), want);
    }
}

#[test]
fn numerals_and_models() {
    let x = int("x");
    let s = solver();
    s.assert(&eq_int(&x, &val(-(1 << 70))));
    assert_eq!(check(&s), Check::Sat);
    let m = s.get_model().unwrap();
    assert_eq!(eval_long(&m, &x), Some(-(1 << 70)));
    let _ = x.get_z3_ast();
}

#[test]
fn the_crates_own_forall_differs_from_z3pys() {
    // Why zz::forall exists: z3::ast::forall_const uses weight 0, which Z3 prints (and searches) differently.
    let (j, a) = (int("j_3"), array("bal"));
    let body = eq_n(&select(&a, &j), 3);
    let ours = forall(&[&j], &body).to_string();
    let crates = z3::ast::forall_const(&[&j], &[], &body).to_string();
    assert_eq!(one_line(ours), "(forall ((j_3 Int)) (= (select bal j_3) 3))");
    assert_ne!(one_line(crates.clone()), one_line(forall(&[&j], &body).to_string()), "{crates}");
}

#[test]
fn comparisons_swap_like_python_operators() {
    // Golden from z3py 5.3.3 with a = Int('a'), n = IntVal(5), m = IntVal(7): Python calls the right operand's
    // reflected method first when it is an IntNumRef (a subclass of ArithRef); arithmetic doesn't swap.
    let (a, n, m) = (int("a"), val(5), val(7));
    let got = [
        gt(&a, &n).to_string(), ge(&a, &n).to_string(), lt(&a, &n).to_string(), le(&a, &n).to_string(),
        eq_int(&a, &n).to_string(), ne_int(&a, &n).to_string(), add(&a, &n).to_string(), sub(&a, &n).to_string(),
        mul(&a, &n).to_string(), div(&a, &n).to_string(), gt(&n, &a).to_string(), eq_int(&n, &m).to_string(),
        ge_n(&a, 0).to_string(),
    ];
    let want = ["(< 5 a)", "(<= 5 a)", "(> 5 a)", "(>= 5 a)", "(= 5 a)", "(distinct 5 a)", "(+ a 5)", "(- a 5)",
                "(* a 5)", "(div a 5)", "(> 5 a)", "(= 5 7)", "(>= a 0)"];
    assert_eq!(got.to_vec(), want.map(String::from).to_vec());
}
