//! The parser against kern/syntax.py: every source in build/oracle_syntax.json (made by
//! `.venv/bin/python validation/oracle_syntax.py build/oracle_syntax.json`) must give the same error text, or the
//! same declarations and, per definition, the same fields, code(), stmt_code(), hash and content_hash.
//! Skipped (with a note) when the dump is missing.

use kern::pyrepr::Py;
use kern::syntax::{code, emit_code, flat, parse_module, stmt_code, Def, Key, Module, Need, Outcome};
use serde_json::{json, Value};

fn py(v: &Py) -> Value {
    match v {
        Py::None => Value::Null,
        Py::Bool(b) => json!(b),
        Py::Int(n) => json!(*n as i64),
        Py::Str(s) => json!(s),
        Py::Tuple(xs) | Py::List(xs) => Value::Array(xs.iter().map(py).collect()),
    }
}

fn key(k: &Key) -> Value {
    py(&k.to_py())
}

fn maps(m: &kern::syntax::Maps) -> Value {
    Value::Array(
        m.iter()
            .map(|(s, kv)| json!([s, Value::Array(kv.iter().map(|(k, v)| json!([key(k), *v as i64])).collect())]))
            .collect(),
    )
}

fn dump_def(d: &Def) -> Value {
    let mut codes: Vec<String> = d
        .needs
        .iter()
        .filter_map(|n| match n {
            Need::Need(c, _, _) => Some(code(c)),
            Need::As(..) => None,
        })
        .collect();
    codes.extend(d.keeps.iter().map(code));
    codes.extend(d.ensures.iter().map(code));
    codes.extend(d.emits.iter().map(emit_code));
    json!({
        "name": d.name,
        "params": Value::Array(d.params.iter().map(|(a, b)| json!([a, b])).collect()),
        "ret": d.ret,
        "needs": Value::Array(d.needs.iter().map(|n| py(&n.to_py())).collect()),
        "keeps": Value::Array(d.keeps.iter().map(|n| py(&n.to_py())).collect()),
        "ensures": Value::Array(d.ensures.iter().map(|n| py(&n.to_py())).collect()),
        "emits": Value::Array(d.emits.iter().map(|n| py(&n.to_py())).collect()),
        "complete": d.complete,
        "ref_origin": Value::Array(d.ref_origin.iter().map(|((k, i), c)| json!([[k, i], c])).collect()),
        "emit_channel": Value::Array(d.emit_channel.iter().map(|(i, c)| json!([i, c])).collect()),
        "body": Value::Array(d.body.iter().map(|n| py(&n.to_py())).collect()),
        "src_lines": Value::Array(d.src_lines.iter().map(|(k, v)| json!([k, v])).collect()),
        "examples": Value::Array(d.examples.iter().map(|e| json!({
            "world": Value::Array(e.world.iter().map(|(a, (b, o))| json!([a, [*b as i64, o]])).collect()),
            "call": json!([e.call.0, Value::Array(e.call.1.iter().map(key).collect())]),
            "outcome": match &e.outcome {
                Outcome::Ok => json!(["ok"]),
                Outcome::Fail(c, v) => json!([c, v.iter().map(|x| *x as i64).collect::<Vec<_>>()]),
            },
            "after": Value::Array(e.after.iter().map(|(a, b)| json!([a, *b as i64])).collect()),
            "src": e.src, "now": e.now as i64, "maps": maps(&e.maps), "after_maps": maps(&e.after_maps),
        })).collect()),
        "assumed": d.assumed,
        "codes": codes,
        "stmts": flat(&d.body).iter().map(|s| stmt_code(s)).collect::<Vec<_>>(),
        "hash": d.hash(),
        "content_hash": d.content_hash(),
    })
}

fn dump(m: &Module) -> Value {
    json!({
        "states": Value::Array(m.states.iter().map(|(a, b)| json!([a, b])).collect()),
        "invariants": Value::Array(m.invariants.iter().map(|(a, b, e)| json!([a, b, py(&e.to_py())])).collect()),
        "channels": m.channels,
        "handlers": Value::Array(m.handlers.values().map(dump_def).collect()),
        "defs": Value::Array(m.defs.values().map(dump_def).collect()),
    })
}

#[test]
fn parser_matches_python() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/oracle_syntax.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("SKIP: no {path}; run validation/oracle_syntax.py");
        return;
    };
    let oracle: Vec<Value> = serde_json::from_str(&text).unwrap();
    let (mut bad, mut defs) = (Vec::new(), 0);
    for o in &oracle {
        let name = o["name"].as_str().unwrap();
        let src = o["src"].as_str().unwrap();
        let got = match parse_module(src, true) {
            Err(e) => json!({ "error": e.0 }),
            Ok(m) => dump(&m),
        };
        if let Some(err) = o.get("error") {
            if got.get("error") != Some(err) {
                bad.push(format!("{name}: want error {err}, got {}", got.get("error").unwrap_or(&json!("a module"))));
            }
            continue;
        }
        if let Some(e) = got.get("error") {
            bad.push(format!("{name}: unexpected error {e}"));
            continue;
        }
        for field in ["states", "invariants", "channels", "handlers", "defs"] {
            if field == "defs" || field == "handlers" {
                let (w, g) = (o[field].as_array().unwrap(), got[field].as_array().unwrap());
                if w.len() != g.len() {
                    bad.push(format!("{name}: {} {field}, want {}", g.len(), w.len()));
                    continue;
                }
                for (wd, gd) in w.iter().zip(g) {
                    defs += 1;
                    for (k, wv) in wd.as_object().unwrap() {
                        if gd.get(k) != Some(wv) {
                            bad.push(format!("{name}: {}.{k}\n  want {wv}\n  got  {}", wd["name"], gd[k]));
                        }
                    }
                }
            } else if o[field] != got[field] {
                bad.push(format!("{name}: {field}\n  want {}\n  got  {}", o[field], got[field]));
            }
        }
    }
    eprintln!("{} sources, {defs} definitions compared, {} mismatches", oracle.len(), bad.len());
    for b in bad.iter().take(30) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
