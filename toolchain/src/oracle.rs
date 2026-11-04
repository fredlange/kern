//! `kern oracle ...`: machine-readable dumps for the Python-vs-Rust diff harness (validation/oracle_analyse.py).
//! Placeholder until kern-8b's version lands (it replaces this whole file).

use std::io::Write;

pub fn main(argv: &[String], _out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let _ = writeln!(err, "kern oracle {}: not available yet", argv.join(" "));
    1
}
