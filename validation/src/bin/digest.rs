//! `digest [--loops] [--dump FILE.json]`: the results digests (validation/README.md).

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(kern_validation::digest::main(&argv));
}
