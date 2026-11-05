//! Synthetic chain modules (port of validation/chains.py, itself a copy of C6's frozen generator), for the S2 digest.
//! Copied from kern-9b's kern/tests/common/mod.rs, which the cache tests use.

/// Synthetic chain modules (port of validation/chains.py, itself a copy of C6's frozen generator): n definitions in
/// chains of depth d; f{c}_1 is a transfer-like leaf, f{c}_j calls f{c}_{j-1}. Every body has k sequential
/// `if amount > T` blocks, every definition e own emits; with `state`, every level increments `cnt.get a`.
pub fn chains(d: usize, k: usize, e: usize, n: usize, state: bool) -> String {
    let mut out: Vec<String> = vec![];
    if state {
        out.extend(["state cnt Account".to_string(), "invariant a:Account cnt.get a >= 0".into(), String::new()]);
    }
    let mut chains: Vec<Vec<usize>> = vec![(1..=d).collect(); n / d];
    if n % d != 0 {
        chains.push((1..=n % d).collect());
    }
    for (c, levels) in chains.iter().enumerate() {
        for &j in levels {
            out.extend(chain_def(c, j, k, e, state));
            out.push(String::new());
        }
    }
    out.join("\n")
}

fn chain_def(c: usize, j: usize, k: usize, e: usize, state: bool) -> Vec<String> {
    let (name, callee) = (format!("f{c}_{j}"), format!("f{c}_{}", j as i64 - 1));
    let cond = k.min(e);
    let mut lines = vec![format!("{name} a:Account b:Account amount:Int")];
    if j == 1 {
        lines.extend([
            "  need amount > 0 else BadAmount".to_string(),
            "  need a != b else SameAccount".into(),
            "  need ledger.balance a >= amount else Insufficient (amount - ledger.balance a)".into(),
        ]);
    } else {
        lines.push(format!("  need as {callee} a b amount"));
    }
    lines.extend([
        "  ok otherwise".to_string(),
        "  keep ledger.total".into(),
        "  ensure ledger.balance' a = ledger.balance a - amount".into(),
        "  ensure ledger.balance' b = ledger.balance b + amount".into(),
    ]);
    if state {
        lines.push(format!("  ensure cnt.get' a = cnt.get a + {j}"));
    }
    lines.extend((0..cond).map(|i| format!("  emit E{i} a amount clock.now when amount > {}", 10 * (i + 1))));
    if j > 1 {
        lines.push(format!("  emit as {callee} a b amount"));
    }
    lines.extend((cond..e).map(|i| format!("  emit F{i} a clock.now")));
    lines.push(format!("  eg A:500@u1 B:0@u2 | {name} A B 100 -> ok | A:400 B:100"));
    lines.push("body".into());
    if state {
        lines.push("  cnt.set a (cnt.get a + 1)".into());
    }
    for i in 0..k {
        lines.push(format!("  if amount > {}", 10 * (i + 1)));
        lines.push(if i < e { format!("    audit.emit (E{i} a amount clock.now)") } else { "    ledger.credit b 0".into() });
    }
    if j == 1 {
        lines.extend(["  ledger.debit a amount".to_string(), "  ledger.credit b amount".into()]);
    } else {
        lines.push(format!("  {callee} a b amount"));
    }
    lines.extend((cond..e).map(|i| format!("  audit.emit (F{i} a clock.now)")));
    lines
}
