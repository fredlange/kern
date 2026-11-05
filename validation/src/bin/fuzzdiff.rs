//! `fuzzdiff [--seed 1] [--loops 300] [--handlers 200] [--no-mutants] [--per-def 100]`: the fuzzer's programs,
//! compiled, against the interpreter (validation/README.md).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kern_validation::fuzzdiff::main(&argv));
}
