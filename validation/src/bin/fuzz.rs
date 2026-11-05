//! `fuzz [--seed 1] [--loops 300] [--handlers 200] [--runs 300] [--no-mutants] [--jobs N] [--fault a|b|c]
//! [--save FILE.json]`: the soundness fuzzer (validation/README.md).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kern_validation::fuzz::main(&argv));
}
