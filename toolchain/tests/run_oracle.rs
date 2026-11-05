//! The interpreter against kern/run.py: every call in build/oracle_run.json (made by
//! `.venv/bin/python validation/oracle_run.py build/oracle_run.json`) must give the same outcome or error (kind and
//! text), world after, last_result, holds, unstated_refusal, broken invariants, observations, loop violations and
//! (Interp::record_heads, the fuzzer's oracle) loop heads.
//! Skipped (with a note) when the dump is missing.

use indexmap::{IndexMap, IndexSet};
use kern::run::{Interp, Outcome, RunError, Value, World};
use kern::syntax::{parse_module, Key};
use serde_json::{json, Value as J};

fn val(j: &J) -> Value {
    match j {
        J::Null => Value::None,
        J::Bool(b) => Value::Bool(*b),
        J::Number(n) => Value::Int(n.as_i64().unwrap() as i128),
        J::String(s) => Value::Str(s.clone()),
        J::Array(xs) => Value::Ctor(xs[0].as_str().unwrap().to_string(), xs[1..].iter().map(val).collect()),
        J::Object(_) => panic!("object value"),
    }
}

fn js(v: &Value) -> J {
    match v {
        Value::None => J::Null,
        Value::Bool(b) => json!(b),
        Value::Int(n) => json!(*n as i64),
        Value::Str(s) => json!(s),
        Value::Ctor(c, a) => {
            let mut xs = vec![json!(c)];
            xs.extend(a.iter().map(js));
            J::Array(xs)
        }
    }
}

fn key(j: &J) -> Key {
    match j {
        J::Number(n) => Key::Int(n.as_i64().unwrap() as i128),
        J::String(s) => Key::Str(s.clone()),
        _ => panic!("bad key"),
    }
}

fn kj(k: &Key) -> J {
    match k {
        Key::Int(n) => json!(*n as i64),
        Key::Str(s) => json!(s),
    }
}

fn world(j: &J) -> World {
    let mut w = World::default();
    for p in j["bal"].as_array().unwrap() {
        w.bal.insert(p[0].as_str().unwrap().into(), p[1].as_i64().unwrap() as i128);
    }
    for p in j["own"].as_array().unwrap() {
        w.own.insert(p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into());
    }
    w.clock = j["clock"].as_i64().unwrap() as i128;
    w.audit = j["audit"].as_array().unwrap().iter().map(val).collect();
    for p in j["maps"].as_array().unwrap() {
        let m = w.maps.entry(p[0].as_str().unwrap().into()).or_default();
        for kv in p[1].as_array().unwrap() {
            m.insert(key(&kv[0]), kv[1].as_i64().unwrap() as i128);
        }
    }
    for p in j["out"].as_array().unwrap() {
        w.out.insert(p[0].as_str().unwrap().into(), p[1].as_array().unwrap().iter().map(val).collect());
    }
    w
}

fn world_json(w: &World) -> J {
    json!({
        "bal": w.bal.iter().map(|(a, b)| json!([a, *b as i64])).collect::<Vec<_>>(),
        "own": w.own.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
        "clock": w.clock as i64,
        "audit": w.audit.iter().map(js).collect::<Vec<_>>(),
        "maps": w.maps.iter().map(|(x, kv)| json!([x, kv.iter().map(|(k, v)| json!([kj(k), *v as i64])).collect::<Vec<_>>()])).collect::<Vec<_>>(),
        "out": w.out.iter().map(|(c, e)| json!([c, e.iter().map(js).collect::<Vec<_>>()])).collect::<Vec<_>>(),
    })
}

fn err_json(e: &RunError) -> J {
    let kind = match e {
        RunError::Kern(_) => "kern".to_string(),
        RunError::Key(_) => "key".to_string(),
        RunError::Type(_) => "type".to_string(),
        RunError::Other(t, _) => t.clone(),
    };
    json!([kind, e.to_string()])
}

fn guarded<T>(r: Result<T, RunError>, f: impl Fn(T) -> J) -> J {
    match r {
        Ok(x) => json!({ "ok": f(x) }),
        Err(e) => json!({ "err": err_json(&e) }),
    }
}

fn broken_json(v: Vec<(String, Value)>) -> J {
    J::Array(v.iter().map(|(t, k)| json!([t, js(k)])).collect())
}

fn one_call(interp: &mut Interp, def: &str, want: &J) -> J {
    let d = interp.defs.defs.get(want.get("call").and_then(|c| c.as_str()).unwrap_or(def)).unwrap().clone();
    let args: Vec<Value> = want["args"].as_array().unwrap().iter().map(val).collect();
    let mut w = world(&want["world"]);
    let before = w.clone();
    let mut rec = serde_json::Map::new();
    let n_obs = interp.observations.len();
    let outcome = match interp.run(&d.name, &args, &mut w) {
        Err(e) => {
            rec.insert("err".into(), err_json(&e));
            rec.insert("after".into(), world_json(&w));
            return J::Object(rec);
        }
        Ok(o) => o,
    };
    rec.insert(
        "outcome".into(),
        match &outcome {
            Outcome::Ok => json!(["ok"]),
            Outcome::Fail(c, a) => json!([c, a.iter().map(js).collect::<Vec<_>>()]),
        },
    );
    rec.insert("after".into(), world_json(&w));
    rec.insert("last_result".into(), interp.last_result.as_ref().map(js).unwrap_or(J::Null));
    rec.insert(
        "observations".into(),
        J::Array(
            interp.observations[n_obs..]
                .iter()
                .map(|(n, a, v)| json!([n, a.iter().map(js).collect::<Vec<_>>(), js(v)]))
                .collect(),
        ),
    );
    if outcome == Outcome::Ok {
        let r = interp.holds(&d, &args, &mut before.clone(), &mut w.clone());
        rec.insert("holds".into(), guarded(r, |v| J::Array(v.iter().map(|(k, c)| json!([k, c])).collect())));
    }
    let r = interp.unstated_refusal(&d, &args, &before, &outcome);
    rec.insert("unstated".into(), guarded(r, |b| json!(b)));
    let r = interp.broken_invariants(&mut w.clone(), None);
    rec.insert("broken".into(), guarded(r, broken_json));
    let mut keys: IndexMap<String, IndexSet<Key>> = IndexMap::new();
    for (t, ks) in want["keys"].as_object().unwrap() {
        keys.insert(t.clone(), ks.as_array().unwrap().iter().map(key).collect());
    }
    rec.insert("keys".into(), want["keys"].clone());
    let r = interp.broken_invariants(&mut w.clone(), Some(&keys));
    rec.insert("broken_keys".into(), guarded(r, broken_json));
    J::Object(rec)
}

#[test]
fn interpreter_matches_python() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../build/oracle_run.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("SKIP: no {path}; run validation/oracle_run.py");
        return;
    };
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let (mut bad, mut n) = (Vec::<String>::new(), 0);
    for m in &oracle {
        let defs = parse_module(m["src"].as_str().unwrap(), true).unwrap();
        for dd in m["defs"].as_array().unwrap() {
            let name = dd["def"].as_str().unwrap();
            let mut interp = Interp::new(&defs);
            let mut loop_interp = Interp::new(&defs);
            loop_interp.check_loops = true;
            loop_interp.budget = Some(2000);
            let mut heads_interp = Interp::new(&defs);
            heads_interp.record_heads = true;
            heads_interp.budget = Some(2000);
            for want in dd["calls"].as_array().unwrap() {
                if want.get("skip").is_some() {
                    continue;
                }
                n += 1;
                let is_loop = want.get("check_loops").is_some();
                let is_heads = want.get("record_heads").is_some();
                let it = if is_heads {
                    &mut heads_interp
                } else if is_loop {
                    &mut loop_interp
                } else {
                    &mut interp
                };
                let mut got = one_call(it, name, want);
                if is_loop {
                    let v: Vec<J> = it.loop_violations.drain(..).map(|(t, k)| json!([t, k as i64])).collect();
                    got.as_object_mut().unwrap().insert("loop_violations".into(), J::Array(v));
                }
                if is_heads {
                    let v: Vec<J> = it.heads.drain(..).map(|(k, bad)| json!([k as i64, bad])).collect();
                    got.as_object_mut().unwrap().insert("heads".into(), J::Array(v));
                }
                for (k, g) in got.as_object().unwrap() {
                    if want.get(k) != Some(g) && bad.len() < 1000 {
                        bad.push(format!(
                            "{} {} args={} {k}\n  want {}\n  got  {}",
                            m["name"], name, want["args"], want.get(k).unwrap_or(&J::Null), g
                        ));
                    }
                }
                for k in ["err", "outcome", "holds", "loop_violations", "heads"] {
                    if want.get(k).is_some() && got.get(k).is_none() {
                        bad.push(format!("{} {} args={}: missing {k}: want {}", m["name"], name, want["args"], want[k]));
                    }
                }
            }
        }
    }
    eprintln!("{} modules, {n} calls compared, {} mismatches", oracle.len(), bad.len());
    for b in bad.iter().take(25) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
