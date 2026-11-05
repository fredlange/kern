//! `c6 [--emit D K E N STATE | --one D K E N STATE]`: C6's frozen runner on the Rust toolchain (validation/README.md).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kern_validation::c6::main(&argv));
}
