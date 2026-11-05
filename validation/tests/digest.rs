//! The results digests are the floor for prover changes (validation/README.md). A changed value is a finding:
//! dump both (`digest [--loops] --dump FILE`) and explain every difference before re-baselining with a dated note.
//!   S2: 3cb0e5efa3b63fb8, the value the Python toolchain froze on 2026-11-04. The Rust toolchain gives it too,
//!       with fresh Z3 contexts (full mutation testing, 10 modules).
//!   loops: e7234c3d8ea6a48e, recorded from Rust on 2026-11-05 (fresh contexts). Python's last value was
//!       c0e94aa224aef288; the one difference is fz2.k's f goal 2, Z3's step-limit borderline (Python: unknown;
//!       Rust: a counterexample that doesn't replay, so conditional).
//!       2026-11-05, S11 (f160617): 776c5ca29063f921. A dump diff against 43362dd shows exactly two goals moved,
//!       fz2.k f and p2.k shifted, both computed-key module invariants: unknown -> runtime (tier ○).
use std::path::Path;

use kern_validation::digest::{digest, digest_loops};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

#[test]
fn the_s2_digest_is_unchanged() {
    assert_eq!(digest(&root()).0, "3cb0e5efa3b63fb8");
}

#[test]
fn the_loop_digest_is_unchanged() {
    assert_eq!(digest_loops(&root()).0, "776c5ca29063f921");
}
