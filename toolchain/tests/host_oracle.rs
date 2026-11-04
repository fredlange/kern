//! The host against kern/host.py: every scenario in build/oracle_host.json (made by `.venv/bin/python
//! validation/oracle_host.py build/oracle_host.json`) is run op for op; every op's result, every sink delivery and
//! every row of every table must be identical. The Rust databases are left in build/host_rs/ so that
//! `validation/oracle_host.py --replay build/host_rs` can replay them with the Python host. Skipped when missing.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use kern::host::{json_value, load_config, value_json, Host, HostError, Options, ProviderFn, SimNet, Sink};
use kern::run::{ProvideError, Value};
use serde_json::{json, Value as J};

fn provider() -> ProviderFn {
    let mut n: i128 = 0;
    Box::new(move |args: &[Value]| {
        n += 1;
        let m = args.first().and_then(|a| a.num()).unwrap_or(0);
        if m <= 0 {
            return Err(ProvideError::Other("ValueError".into(), "Upper bound must be positive.".into()));
        }
        Ok(Value::Int((n * 7) % m.max(1)))
    })
}

fn answers() -> Box<dyn FnMut(&Value) -> J> {
    let mut n = 0;
    Box::new(move |_| {
        n += 1;
        [json!({"value": (n * 37) % 901}), json!({"value": -1}), json!({"value": "x"}), json!({}), json!({"value": true})]
            [n % 5]
            .clone()
    })
}

fn exception(e: &HostError) -> J {
    match e {
        HostError::NoDef(n) => json!(["exception", "KeyError", format!("'{n}'")]),
        HostError::Unproven(n) => json!(["exception", "Unproven", n]),
        HostError::Fault(t, m) => json!(["exception", t, m]),
    }
}

fn tables(db: &rusqlite::Connection) -> J {
    let mut out = serde_json::Map::new();
    for t in ["meta", "account", "cell", "log", "outbox"] {
        let mut st = db.prepare(&format!("select * from {t} order by rowid")).unwrap();
        let n = st.column_count();
        let rows: Vec<J> = st
            .query_map([], |r| {
                Ok(J::Array(
                    (0..n)
                        .map(|i| match r.get_ref(i).unwrap() {
                            rusqlite::types::ValueRef::Integer(x) => json!(x),
                            rusqlite::types::ValueRef::Text(s) => json!(String::from_utf8_lossy(s)),
                            rusqlite::types::ValueRef::Null => J::Null,
                            other => json!(format!("{other:?}")),
                        })
                        .collect(),
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        out.insert(t.into(), J::Array(rows));
    }
    J::Object(out)
}

fn replies(rs: Vec<(String, Vec<Value>, kern::host::CallResult)>) -> J {
    J::Array(rs.into_iter().map(|(n, a, r)| json!([n, a.iter().map(value_json).collect::<Vec<_>>(), r])).collect())
}

#[test]
fn host_matches_python() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let Ok(text) = std::fs::read_to_string(format!("{root}/build/oracle_host.json")) else {
        eprintln!("SKIP: no build/oracle_host.json; run validation/oracle_host.py");
        return;
    };
    let dbs = format!("{root}/build/host_rs");
    std::fs::create_dir_all(&dbs).unwrap();
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let (mut bad, mut n_ops, mut n_cross) = (Vec::<String>::new(), 0, 0);
    for sc in oracle.iter().filter(|s| s.get("ops").is_some()) {
        let name = sc["name"].as_str().unwrap();
        let defs = kern::package::load(sc["path"].as_str().unwrap(), &IndexMap::new()).unwrap();
        let delivered: Rc<RefCell<Vec<J>>> = Rc::new(RefCell::new(vec![]));
        let sink = |ch: &str| -> Sink {
            let (d, ch) = (delivered.clone(), ch.to_string());
            Box::new(move |seq, ev| d.borrow_mut().push(json!([seq, ch, value_json(ev)])))
        };
        let mut tick = 999;
        let mut opts = Options {
            db: format!("{dbs}/{name}.db"),
            clock: Some(Box::new(move || {
                tick += 1;
                tick
            })),
            sinks: Some(vec![sink("audit")]),
            channel_sinks: Some(defs.channels.iter().map(|c| (c.clone(), vec![sink(c)])).collect()),
            gate: false,
            providers: Some(IndexMap::from([("rand.below".to_string(), provider())])),
            ..Default::default()
        };
        if let Some(cfg) = sc.get("config").and_then(|c| c.as_str()) {
            opts.endpoints = Some(load_config(cfg, &defs).unwrap());
            opts.net = Some(Box::new(SimNet::new(answers(), sc["seed"].as_i64().unwrap() as i128)));
        }
        let _ = std::fs::remove_file(&opts.db);
        let mut h = Host::new(&defs, IndexMap::new(), opts).unwrap();
        h.init(sc["world"].as_str().unwrap()).unwrap();
        for (i, (op, want)) in sc["ops"].as_array().unwrap().iter().zip(sc["results"].as_array().unwrap()).enumerate() {
            n_ops += 1;
            let got = match op[0].as_str().unwrap() {
                "call" => {
                    let args: Vec<Value> = op[2].as_array().unwrap().iter().map(json_value).collect();
                    h.call(op[1].as_str().unwrap(), &args).map(J::Object).unwrap_or_else(|e| exception(&e))
                }
                "flush" => h.flush().map(|n| json!(n)).unwrap_or_else(|e| exception(&e)),
                "pump" => h.pump(vec![]).map(replies).unwrap_or_else(|e| exception(&e)),
                "release" => {
                    let held = h.net.as_mut().unwrap().release();
                    h.pump(held).map(replies).unwrap_or_else(|e| exception(&e))
                }
                "replay" => h.replay().map(|b| json!(b)).unwrap_or_else(|e| exception(&e)),
                "state" => {
                    let w = h.load().unwrap();
                    json!({
                        "bal": w.bal.iter().map(|(a, b)| (a.clone(), json!(*b as i64))).collect::<serde_json::Map<_, _>>(),
                        "own": w.own.iter().map(|(a, o)| (a.clone(), json!(o))).collect::<serde_json::Map<_, _>>(),
                        "maps": w.maps.iter().map(|(s, m)| json!([s, m.iter().map(|(k, v)| json!([match k {
                            kern::syntax::Key::Int(n) => json!(*n as i64), kern::syntax::Key::Str(s) => json!(s) }, *v as i64])).collect::<Vec<_>>()])).collect::<Vec<_>>(),
                    })
                }
                "sql" => {
                    h.db.execute_batch(op[1].as_str().unwrap()).unwrap();
                    J::Null
                }
                other => panic!("op {other}"),
            };
            if &got != want {
                bad.push(format!("{name} op {i} {op}\n  want {want}\n  got  {got}"));
            }
        }
        // the other direction: this host replays the Python host's database to the verdict Python gave
        let py_db = format!("{root}/build/host_py/{name}.db");
        let last_replay = sc["ops"].as_array().unwrap().iter().zip(sc["results"].as_array().unwrap())
            .filter(|(op, _)| op[0] == "replay").last().map(|(_, r)| r.clone());
        if let (true, Some(want)) = (std::path::Path::new(&py_db).exists(), last_replay) {
            let mut o = Options { db: py_db.clone(), sinks: Some(vec![]), gate: false,
                providers: Some(IndexMap::from([("rand.below".to_string(), provider())])), ..Default::default() };
            if let Some(cfg) = sc.get("config").and_then(|c| c.as_str()) {
                o.endpoints = Some(load_config(cfg, &defs).unwrap());
                o.net = Some(Box::new(SimNet::new(answers(), 0)));
            }
            match Host::new(&defs, IndexMap::new(), o) {
                Ok(other) => {
                    let got = other.replay().map(|b| json!(b)).unwrap_or_else(|e| exception(&e));
                    if got != want {
                        bad.push(format!("{name}: replay of the Python database\n  want {want}\n  got  {got}"));
                    }
                    n_cross += 1;
                }
                Err(e) => bad.push(format!("{name}: cannot open the Python database: {}", e.0)),
            }
        }
        let want_d = &sc["delivered"];
        if J::Array(delivered.borrow().clone()) != *want_d {
            bad.push(format!("{name}: deliveries differ ({} vs {})", delivered.borrow().len(), want_d.as_array().unwrap().len()));
        }
        let got_t = tables(&h.db);
        for t in ["meta", "account", "cell", "log", "outbox"] {
            if got_t[t] != sc["tables"][t] {
                let (w, g) = (sc["tables"][t].as_array().unwrap(), got_t[t].as_array().unwrap());
                let first = w.iter().zip(g).position(|(a, b)| a != b).unwrap_or(w.len().min(g.len()));
                bad.push(format!("{name}: table {t} differs ({} vs {} rows), first at row {first}:\n  want {}\n  got  {}",
                    w.len(), g.len(), w.get(first).unwrap_or(&J::Null), g.get(first).unwrap_or(&J::Null)));
            }
        }
    }
    eprintln!("{} scenarios, {n_ops} ops compared, {n_cross} Python databases replayed, {} mismatches",
              oracle.iter().filter(|s| s.get("ops").is_some()).count(), bad.len());
    for b in bad.iter().take(15) {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}

#[test]
fn http_front_end_matches_python() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let Ok(text) = std::fs::read_to_string(format!("{root}/build/oracle_host.json")) else {
        eprintln!("SKIP: no build/oracle_host.json; run validation/oracle_host.py");
        return;
    };
    let oracle: Vec<J> = serde_json::from_str(&text).unwrap();
    let sc = oracle.iter().find(|s| s["name"] == "http").expect("an http scenario");
    let defs = kern::package::load(sc["path"].as_str().unwrap(), &IndexMap::new()).unwrap();
    let mut tick = 999;
    let opts = Options {
        clock: Some(Box::new(move || {
            tick += 1;
            tick
        })),
        sinks: Some(vec![]),
        gate: false,
        ..Default::default()
    };
    let mut h = Host::new(&defs, IndexMap::new(), opts).unwrap();
    h.init(sc["world"].as_str().unwrap()).unwrap();
    let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = srv.server_addr().to_ip().unwrap().port();
    let reqs: Vec<(String, String, Option<String>)> = sc["http"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| (q[0].as_str().unwrap().into(), q[1].as_str().unwrap().into(), q[2].as_str().map(String::from)))
        .collect();
    let n = reqs.len();
    let client = std::thread::spawn(move || {
        reqs.into_iter()
            .map(|(m, p, body)| {
                let req = ureq::request(&m, &format!("http://127.0.0.1:{port}{p}"));
                let r = match body {
                    Some(b) => req.send_string(&b),
                    None => req.call(),
                };
                let resp = match r {
                    Ok(resp) => resp,
                    Err(ureq::Error::Status(_, resp)) => resp,
                    Err(e) => panic!("{e}"),
                };
                json!([resp.status(), resp.into_string().unwrap()])
            })
            .collect::<Vec<J>>()
    });
    for _ in 0..n {
        let req = srv.recv().unwrap();
        kern::host::handle_http(&mut h, req).unwrap();
    }
    let got = client.join().unwrap();
    let want = sc["responses"].as_array().unwrap();
    let bad: Vec<String> = got
        .iter()
        .zip(want)
        .zip(sc["http"].as_array().unwrap())
        .filter(|((g, w), _)| g != w)
        .map(|((g, w), q)| format!("{q}\n  want {w}\n  got  {g}"))
        .collect();
    eprintln!("{n} HTTP requests compared, {} mismatches", bad.len());
    for b in &bad {
        eprintln!("{b}");
    }
    assert!(bad.is_empty());
}
