//! `difftest [--per-def N] [--seed S] [MODULE.k ...]`: the compiled program against the interpreter (validation/README.md).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kern_validation::difftest::main(&argv));
}
