//! A tool change invalidates every cached result (test_fundamentals.py's s2_a_tool_change_invalidates_everything,
//! which patched report._TOOL). KERN_TOOL_SALT stands in for a different checker build in report::cache_key. An env
//! var is process-global, so this test lives alone in its own test binary.

mod common;
use common::{tmp, ROOT};

use kern::report::analyse;
use kern::syntax::parse_module;

#[test]
fn s2_a_tool_change_invalidates_everything() {
    let src = std::fs::read_to_string(format!("{ROOT}/examples/escrow/escrow_s1.k")).unwrap();
    let dir = tmp("tool_salt");
    let cache = dir.to_string_lossy().to_string();
    let defs = parse_module(&src, true).unwrap();
    analyse(&defs, false, None, Some(&cache)).unwrap();
    let warm = analyse(&defs, false, None, Some(&cache)).unwrap();
    assert!(warm.values().all(|r| r.cached), "the second run should be served from the cache");
    std::env::set_var("KERN_TOOL_SALT", "a different prover");
    let after = analyse(&defs, false, None, Some(&cache)).unwrap();
    assert!(!after.values().any(|r| r.cached));
}
