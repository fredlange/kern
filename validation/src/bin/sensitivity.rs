//! `sensitivity [--per-def N] [--dir DIR]`: plants known bugs in kern-rt and the emitter, one at a time, in a copy of
//! the workspace, and checks that difftest catches each (validation/README.md).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kern_validation::sensitivity::main(&argv));
}
