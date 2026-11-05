//! The host (port of tests/test_host.py): the proofs' assumptions survive contact with SQLite, a wall clock, an
//! outbox and HTTP. Design and caveats: docs/HOST.md.

mod common;
use common::{tmp, ROOT};

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use kern::host::{host_facts, load_config, manifest_text, CallResult, Host, HostError, Options, SimNet, Sink};
use kern::pyrandom::Random;
use kern::run::{Interp, Outcome, Value, World};
use kern::syntax::{parse_module, parse_world, split_maps, Key, Module};
use serde_json::{json, Value as J};

const WORLD: &str = "A:1000@u1 C:300@u3 E:0@u0 B:0@u2 H:0@u9";

fn module(src: &str) -> Module {
    let defs = parse_module(src, true).unwrap();
    kern::wf::check_module(&defs).unwrap();
    defs
}

fn escrow() -> Module {
    module(&std::fs::read_to_string(format!("{ROOT}/examples/escrow/escrow_s2.k")).unwrap())
}

fn limits() -> Module {
    module(&std::fs::read_to_string(format!("{ROOT}/examples/limits/limits.k")).unwrap())
}

fn reports(defs: &Module) -> IndexMap<String, kern::report::Report> {
    kern::report::analyse(defs, false, None, None).unwrap()
}

fn ticks() -> Box<dyn FnMut() -> i128> {
    let mut t = 999;
    Box::new(move || {
        t += 1;
        t
    })
}

fn opts() -> Options {
    Options { clock: Some(ticks()), sinks: Some(vec![]), ..Default::default() }
}

fn host<'m>(defs: &'m Module, world: &str, o: Options) -> Host<'m> {
    let mut h = Host::new(defs, reports(defs), o).unwrap();
    h.init(world).unwrap();
    h
}

fn mem_world(world: &str) -> World {
    let (rest, maps) = split_maps(world).unwrap();
    World::from_example(&parse_world(&rest, None).unwrap(), 0, &maps)
}

fn s(x: &str) -> Value {
    Value::Str(x.into())
}

fn i(n: i128) -> Value {
    Value::Int(n)
}

fn call(h: &mut Host, name: &str, args: &[Value]) -> CallResult {
    h.call(name, args).unwrap()
}

#[test]
fn sqlite_host_matches_the_interpreter() {
    // Same calls, same clock: SQLite rollback must give exactly the interpreter's restore semantics.
    let defs = escrow();
    let mut rng = Random::from_int(7);
    let mut h = host(&defs, WORLD, opts());
    let mut w = mem_world(WORLD);
    let mut interp = Interp::new(&defs);
    let served: Vec<&kern::syntax::Def> = defs.defs.values().filter(|d| h.reports[&d.name].clean).collect();
    let (mut counts, mut events) = (IndexMap::<String, u32>::new(), vec![]);
    for n in 0..600 {
        let accounts: Vec<String> = w.bal.keys().cloned().collect();
        let d = *rng.choice(&served);
        let args: Vec<Value> = d
            .params
            .iter()
            .map(|(_, t)| {
                if t == "Account" {
                    s(rng.choice(&accounts))
                } else {
                    i(*rng.choice(&[0, 1, 2, 3, 5, 50, 99, 100, 250, -1]))
                }
            })
            .collect();
        let r = call(&mut h, &d.name, &args);
        w.clock = 1000 + n;
        w.audit.clear();
        let outcome = interp.run(&d.name, &args, &mut w).unwrap();
        let want = if outcome == Outcome::Ok { "ok" } else { "refused" };
        assert_eq!(r["status"], want, "{n} {} {args:?} {r:?}", d.name);
        if let Outcome::Fail(c, a) = &outcome {
            assert_eq!(r["refused"], json!(c));
            assert_eq!(r["args"], J::Array(a.iter().map(kern::host::value_json).collect()));
        }
        events.extend(w.audit.iter().cloned());
        *counts.entry(want.into()).or_default() += 1;
    }
    let db = h.load().unwrap();
    assert_eq!(db.bal, w.bal);
    let nonempty: IndexMap<String, IndexMap<Key, i128>> =
        w.maps.iter().filter(|(_, m)| !m.is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(db.maps, nonempty);
    let stored: Vec<J> = h
        .db
        .prepare("select event from outbox order by seq")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|e| serde_json::from_str(&e.unwrap()).unwrap())
        .collect();
    assert_eq!(stored, events.iter().map(kern::host::value_json).collect::<Vec<_>>());
    assert!(counts["ok"] >= 100 && counts["refused"] >= 100, "{counts:?}"); // not vacuous
    assert!(h.replay().unwrap().is_empty());
}

#[test]
fn refusal_writes_nothing_but_is_logged() {
    let defs = escrow();
    let mut h = host(&defs, WORLD, opts());
    let r = call(&mut h, "transfer", &[s("E"), s("A"), i(5)]);
    assert!(r["status"] == "refused" && r["refused"] == "Insufficient");
    assert_eq!(h.load().unwrap().bal["A"], 1000);
    let n: i64 = h.db.query_row("select count(*) from outbox", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    let log: (String, String) = h.db.query_row("select name, outcome from log", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(log, ("transfer".into(), r#"{"status": "refused", "refused": "Insufficient", "args": [5]}"#.into()));
}

#[test]
fn bad_call_is_an_error_not_a_refusal() {
    let defs = escrow();
    let mut h = host(&defs, WORLD, opts());
    assert_eq!(call(&mut h, "transfer", &[s("A"), s("Nobody"), i(5)])["status"], "error");
    assert_eq!(call(&mut h, "transfer", &[s("A"), s("B"), Value::Bool(true)])["status"], "error"); // JSON true is not an Int
    assert_eq!(h.load().unwrap().bal, mem_world(WORLD).bal);
}

const BURN: &str = "invariant a:Account ledger.balance a >= 0

burn a:Account n:Int
body
  ledger.debit a n
";

#[test]
fn gate_blocks_unproven_definitions() {
    let defs = module(BURN);
    let mut h = host(&defs, "A:10@u1", opts());
    assert!(!h.reports["burn"].clean);
    assert!(matches!(h.call("burn", &[s("A"), i(5)]), Err(HostError::Unproven(_))));
}

#[test]
fn runtime_invariant_check_rolls_back() {
    // Tier ○: with the gate off, an invariant the prover did not establish is checked per call.
    let defs = module(BURN);
    let mut h = host(&defs, "A:10@u1", Options { gate: false, ..opts() });
    assert_eq!(call(&mut h, "burn", &[s("A"), i(5)])["status"], "ok");
    let r = call(&mut h, "burn", &[s("A"), i(50)]);
    assert!(r["status"] == "error" && r["error"].as_str().unwrap().contains("invariant"));
    assert_eq!(h.load().unwrap().bal["A"], 5);
}

#[test]
fn delivery_is_at_least_once() {
    // A crash between delivering an event and marking it delivered sends it again; consumers dedupe on seq.
    let defs = escrow();
    let got: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(vec![]));
    let g = got.clone();
    let flaky: Sink = Box::new(move |seq, _| {
        g.borrow_mut().push(seq);
        if g.borrow().len() == 2 {
            panic!("crash after delivering");
        }
    });
    let mut h = host(&defs, WORLD, Options { sinks: Some(vec![flaky]), ..opts() });
    call(&mut h, "transfer", &[s("A"), s("B"), i(5)]); // emits Moved: seq 1
    call(&mut h, "tip", &[s("A"), s("B"), i(5)]); // emits Moved, Tipped: seq 2, 3
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.flush()));
    assert!(crashed.is_err());
    let g = got.clone();
    h.sinks = vec![Box::new(move |seq, _| g.borrow_mut().push(seq))];
    assert_eq!(h.flush().unwrap(), 2);
    assert_eq!(*got.borrow(), [1, 2, 2, 3]); // seq 2 delivered twice
}

#[test]
fn replay_detects_a_tampered_database() {
    let defs = escrow();
    let mut h = host(&defs, WORLD, opts());
    call(&mut h, "transfer", &[s("A"), s("B"), i(5)]);
    assert!(h.replay().unwrap().is_empty());
    h.db.execute("update account set balance = balance + 1 where name='B'", []).unwrap();
    assert!(h.replay().unwrap().iter().any(|m| m.contains("state differs")));
}

#[test]
fn console_repl() {
    let defs = escrow();
    let mut h = host(&defs, WORLD, opts());
    let mut out = vec![];
    let lines = ["transfer A B 5", "transfer A B 5000", "nope", "state"].iter().map(|l| l.to_string());
    kern::host::repl(&mut h, lines, &mut |l| out.push(l)).unwrap();
    assert_eq!(out[0], "ok");
    assert_eq!(out[1], "refused Insufficient 4005");
    assert!(out[2].starts_with("error"));
    let st: J = serde_json::from_str(&out[3]).unwrap();
    assert_eq!(st["balances"]["B"], 5);
}

/// Serve `n` requests made by `client` (on another thread) through the host's HTTP front end.
fn serve_requests<T: Send + 'static>(h: &mut Host, n: usize, client: impl FnOnce(String) -> T + Send + 'static) -> T {
    let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", srv.server_addr().to_ip().unwrap().port());
    let t = std::thread::spawn(move || client(base));
    for _ in 0..n {
        let req = srv.recv().unwrap();
        kern::host::handle_http(h, req).unwrap();
    }
    t.join().unwrap()
}

fn post(base: &str, name: &str, body: J) -> (u16, J) {
    let r = ureq::post(&format!("{base}/{name}")).send_string(&body.to_string());
    let resp = match r {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("{e}"),
    };
    (resp.status(), serde_json::from_str(&resp.into_string().unwrap()).unwrap())
}

fn get(base: &str, path: &str) -> J {
    serde_json::from_str(&ureq::get(&format!("{base}{path}")).call().unwrap().into_string().unwrap()).unwrap()
}

#[test]
fn http() {
    let defs = escrow();
    let mut h = host(&defs, WORLD, opts());
    let out = serve_requests(&mut h, 5, |base| {
        let a = post(&base, "transfer", json!({"from": "A", "to": "B", "amount": 5})).0;
        let (code, body) = post(&base, "transfer", json!({"from": "A", "to": "B", "amount": 5000}));
        let c = post(&base, "transfer", json!({"from": "A", "amount": 5})).0;
        let d = post(&base, "nope", json!({})).0;
        let m = get(&base, "/manifest");
        (a, code, body, c, d, m)
    });
    assert_eq!(out.0, 200);
    assert_eq!((out.1, out.2["refused"].clone(), out.2["args"].clone()), (409, json!("Insufficient"), json!([4005])));
    assert_eq!(out.3, 400);
    assert_eq!(out.4, 404);
    assert_eq!(out.5["transfer"]["uses"], json!(["audit", "clock", "ledger.write"]));
}

#[test]
fn genesis_world_must_satisfy_invariants() {
    let defs = escrow();
    let mut h = Host::new(&defs, reports(&defs), opts()).unwrap();
    let e = h.init("A:10@u1 hold.7:-1").unwrap_err();
    assert!(e.contains("invariant"), "{e}");
    assert!(h.init("A:10@u1").unwrap());
}

// --- H1 (docs/research/HOST_PLAN.md): lazy world, key-scoped invariant checks

static STATEMENTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_statement(_: &str) {
    STATEMENTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

fn statements_per_call(defs: &Module, n_accounts: usize) -> usize {
    let mut h = Host::new(defs, reports(defs), Options { clock: Some(Box::new(|| 1)), sinks: Some(vec![]), ..Default::default() }).unwrap();
    h.init(&(0..n_accounts).map(|i| format!("A{i}:1000@u1")).collect::<Vec<_>>().join(" ")).unwrap();
    // count statements through SQLite's trace hook while the call runs (between calls the host holds the only handle)
    Rc::get_mut(&mut h.db).unwrap().trace(Some(count_statement));
    STATEMENTS.store(0, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(call(&mut h, "transfer", &[s("A0"), s("A1"), i(5)])["status"], "ok");
    let n = STATEMENTS.load(std::sync::atomic::Ordering::SeqCst);
    Rc::get_mut(&mut h.db).unwrap().trace(None);
    n
}

#[test]
fn a_call_does_not_read_the_whole_world() {
    // the statements a call runs don't grow with the world
    let defs = escrow();
    let (small, big) = (statements_per_call(&defs, 50), statements_per_call(&defs, 5000));
    assert!(small > 0);
    assert_eq!(small, big);
}

#[test]
fn lazy_total_sees_this_calls_writes() {
    let defs = module("pay to:Account amount:Int\nbody\n  ledger.credit to amount\n  audit.emit (Total ledger.total)\n");
    let mut h = host(&defs, "A:10@u1 B:5@u2", opts());
    let mut w = mem_world("A:10@u1 B:5@u2");
    let r = call(&mut h, "pay", &[s("A"), i(7)]);
    w.clock = 1000;
    Interp::new(&defs).run("pay", &[s("A"), i(7)], &mut w).unwrap();
    assert_eq!(r["events"], json!([["Total", 22]]));
    assert_eq!(w.audit, vec![Value::Ctor("Total".into(), vec![i(22)])]);
}

#[test]
fn key_scoped_invariant_check_catches_a_write_at_any_key() {
    let defs = module("state hold Int\ninvariant o:Int hold.get o >= 0\n\npoke id:Int\nbody\n  hold.set (id + 1) (0 - 5)\n");
    let mut h = host(&defs, "A:1@u1", Options { gate: false, ..opts() });
    let r = call(&mut h, "poke", &[i(3)]);
    let e = r["error"].as_str().unwrap();
    assert!(r["status"] == "error" && e.contains("invariant") && e.contains("at 4"), "{e}");
    assert!(h.load().unwrap().maps.is_empty());
}

#[test]
fn invariants_read_state_only_at_their_own_key() {
    // checking invariants key by key is only sound if they read nothing else
    for inv in ["a:Account ledger.total >= 0", "o:Int hold.get 3 >= 0", "o:Int hold.get (o + 1) >= 0",
                "a:Account clock.now >= 0 or ledger.balance a >= 0"] {
        let defs = parse_module(&format!("state hold Int\ninvariant {inv}\n"), true).unwrap();
        let e = kern::wf::check_module(&defs).unwrap_err().message();
        assert!(e.contains("must read state at"), "{inv}: {e}");
    }
}

// --- H2: named output channels

const CH: &str = "channel console

hello to:Account amount:Int
  need amount > 0 else BadAmount
  ok otherwise
  ensure ledger.balance' to = ledger.balance to + amount
  emit console Line to amount
  emit Credited to amount
body
  ledger.credit to amount
  audit.emit (Credited to amount)
  console.emit (Line to amount)
";

fn emit_goals(src: &str, name: &str) -> Vec<kern::report::Guarantee> {
    let defs = module(src);
    reports(&defs)[name].guarantees.iter().filter(|g| g.kind == "emit").cloned().collect()
}

#[test]
fn channel_spec_is_an_exact_sequence_per_channel() {
    let st: Vec<&str> = emit_goals(CH, "hello").iter().map(|g| g.status.as_str()).collect();
    assert_eq!(st, ["proven", "proven"]);
    let wrong = emit_goals(&CH.replace("console.emit (Line to amount)", "console.emit (Line to 0)"), "hello");
    let g = wrong.iter().find(|g| g.clause.starts_with("console")).unwrap();
    assert!(g.status.as_str() == "refuted" && g.counterexample.as_ref().unwrap().confirmed_by_interpreter);
    let others: Vec<&str> = wrong.iter().filter(|g| !g.clause.starts_with("console")).map(|g| g.status.as_str()).collect();
    assert_eq!(others, ["proven"]); // audit unaffected
}

#[test]
fn a_channel_the_callee_spec_omits_is_opaque_to_callers() {
    let src = CH.replace("  emit console Line to amount\n", "")
        + "\ntwice to:Account amount:Int\n  emit console Line to amount\nbody\n  hello to amount\n";
    let gs = emit_goals(&src, "twice");
    let g = gs.iter().find(|g| g.clause.starts_with("console")).unwrap();
    assert!(matches!(g.status.as_str(), "unknown" | "tested"));
    assert!(g.reason.as_deref().unwrap_or("").contains("does not say what it emits"));
}

#[test]
fn channels_must_be_declared_and_are_not_readable() {
    let wf_err = |src: &str| kern::wf::check_module(&parse_module(src, true).unwrap()).is_err();
    assert!(wf_err("f a:Account\n  emit radio Ping a\nbody\n  ledger.credit a 1\n"));
    assert!(parse_module("channel audit\n", true).is_err());
    assert!(wf_err("channel console\n\nf a:Account\n  ensure console.emit (Ping a)\nbody\n  console.emit (Ping a)\n"));
}

#[test]
fn audit_only_code_keeps_its_hash() {
    // channels change nothing for existing code (hash of escrow_s1 transfer, before H2)
    let defs = parse_module(&std::fs::read_to_string(format!("{ROOT}/examples/escrow/escrow_s1.k")).unwrap(), true).unwrap();
    assert_eq!(defs.defs["transfer"].hash(), "27367a3f");
}

#[test]
fn host_delivers_channels_after_commit_and_replays_them() {
    let defs = module(CH);
    let (lines, audit): (Rc<RefCell<Vec<Value>>>, Rc<RefCell<Vec<Value>>>) = Default::default();
    let (l, a) = (lines.clone(), audit.clone());
    let o = Options {
        sinks: Some(vec![Box::new(move |_, e| a.borrow_mut().push(e.clone()))]),
        channel_sinks: Some(IndexMap::from([("console".to_string(), vec![Box::new(move |_, e: &Value| l.borrow_mut().push(e.clone())) as Sink])])),
        ..opts()
    };
    let mut h = host(&defs, "A:0@u1", o);
    assert_eq!(call(&mut h, "hello", &[s("A"), i(5)])["out"], json!({"console": [["Line", "A", 5]]}));
    assert_eq!(call(&mut h, "hello", &[s("A"), i(0)])["status"], "refused"); // rolled back: nothing queued
    assert!(lines.borrow().is_empty());
    assert_eq!(h.flush().unwrap(), 2);
    assert_eq!(*lines.borrow(), vec![Value::Ctor("Line".into(), vec![s("A"), i(5)])]);
    assert_eq!(*audit.borrow(), vec![Value::Ctor("Credited".into(), vec![s("A"), i(5)])]);
    let chans: Vec<String> =
        h.db.prepare("select channel from outbox order by seq").unwrap().query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    assert_eq!(chans, ["audit", "console"]);
    assert!(h.replay().unwrap().is_empty());
    h.db.execute("update outbox set event = ? where channel = 'console'", [r#"["Line", "A", 6]"#]).unwrap();
    assert!(h.replay().unwrap().iter().any(|m| m.contains("differs")));
}

// --- H3/H4: requests are events, replies are calls (examples/limits/limits.k, limits.host)

const CONFIG: &str = "Fetch -> GET http://127.0.0.1:{port}/limit/{1}; reply got_limit {1} {2} $.value; fail limit_failed {1} {2}";
const LIMITS_WORLD: &str = "A:500@u1 B:500@u2 C:500@u3 K:0@u9";

fn limits_host<'m>(defs: &'m Module, reps: &IndexMap<String, kern::report::Report>, net: Option<Box<dyn kern::host::Net>>, port: u16) -> Host<'m> {
    let eps = load_config(&CONFIG.replace("{port}", &port.to_string()), defs).unwrap();
    let mut h = Host::new(defs, reps.clone(), Options { endpoints: Some(eps), net, ..opts() }).unwrap();
    h.init(LIMITS_WORLD).unwrap();
    h
}

#[test]
fn hostile_network_cannot_break_the_invariants() {
    // item 5: seeded runs; replies dropped, duplicated, delayed, reordered, failed or garbled
    let defs = limits();
    let reps = reports(&defs);
    let mut st: IndexMap<&str, u64> = IndexMap::new();
    for run in 0..300 {
        let rng = Rc::new(RefCell::new(Random::from_int(run)));
        let r2 = rng.clone();
        let values = Box::new(move |_: &Value| {
            let mut r = r2.borrow_mut();
            let v = r.randint(0, 900);
            let opts = [json!({"value": v}), json!({"value": -1}), json!({"value": "x"}), json!({})];
            r.choice(&opts).clone()
        });
        let mut h = limits_host(&defs, &reps, Some(Box::new(SimNet::new(values, run))), 1);
        let (mut applied, mut latest) = (std::collections::BTreeSet::<(String, i128)>::new(), IndexMap::<String, i128>::new());
        for _ in 0..25 {
            let a = rng.borrow_mut().choice(&["A", "B", "C"]).to_string();
            let op = rng.borrow_mut().random();
            let r = if op < 0.35 {
                let r = call(&mut h, "request_limit", &[s(&a)]);
                if r["status"] == "ok" {
                    *latest.entry(a.clone()).or_default() += 1;
                }
                r
            } else if op < 0.65 {
                let amount = *rng.borrow_mut().choice(&[1, 50, 200, 600]);
                call(&mut h, "withdraw", &[s(&a), s("K"), i(amount)])
            } else if op < 0.75 {
                call(&mut h, "reset_day", &[s(&a)])
            } else {
                CallResult::new()
            };
            *st.entry("calls").or_default() += 1;
            *st.entry("faults").or_default() += (r.get("fault") == Some(&json!("module"))) as u64;
            h.flush().unwrap();
            let held = if rng.borrow_mut().random() < 0.5 { h.net.as_mut().unwrap().release() } else { vec![] };
            for (_, args, res) in h.pump(held).unwrap() {
                *st.entry("replies").or_default() += 1;
                *st.entry("faults").or_default() += (res.get("fault") == Some(&json!("module"))) as u64;
                let key = (args[0].str(), args[1].num().unwrap());
                if applied.contains(&key) || Some(&key.1) != latest.get(&key.0) {
                    *st.entry("dup_or_old").or_default() += 1;
                    *st.entry("dup_refused").or_default() += (res.get("refused") == Some(&json!("Stale"))) as u64;
                } else if res["status"] == "ok" {
                    applied.insert(key);
                    *st.entry("applied").or_default() += 1;
                }
            }
        }
        *st.entry("replay_mismatch").or_default() += (!h.replay().unwrap().is_empty()) as u64;
        *st.entry("runs").or_default() += 1;
    }
    eprintln!("{st:?}");
    let g = |k: &str| st.get(k).copied().unwrap_or(0);
    assert_eq!(g("faults"), 0);
    assert_eq!(g("replay_mismatch"), 0);
    assert!(g("dup_or_old") > 150 && g("dup_refused") == g("dup_or_old"));
    assert!(g("applied") > 300); // not vacuous
}

#[test]
fn config_is_checked_at_startup() {
    // a request with no endpoint, an endpoint nobody sends, or a reply def with the wrong types are refused
    let defs = limits();
    let e = Host::new(&defs, reports(&defs), opts()).err().unwrap().0;
    assert!(e.contains("no host config"), "{e}");
    let err = |text: &str| load_config(text, &defs).unwrap_err().0;
    assert!(err("").contains("has no endpoint"));
    assert!(err(&format!("{CONFIG}\nPing -> GET http://x/; reply got_limit {{1}} {{2}} $.v; fail limit_failed {{1}} {{2}}")).contains("never sends"));
    assert!(err("Fetch -> GET http://x/{1}; reply got_limit {1} $.value; fail limit_failed {1} {2}").contains("takes"));
    assert!(err("Fetch -> GET http://x/{1}; reply nope {1}; fail limit_failed {1} {2}").contains("does not exist"));
}

#[test]
fn manifest_lists_every_endpoint() {
    let defs = limits();
    let reps = reports(&defs);
    let h = limits_host(&defs, &reps, Some(Box::new(SimNet::new(Box::new(|_| json!({"value": 1})), 0))), 1);
    assert!(manifest_text(&h).contains("net: Fetch -> GET http://127.0.0.1:1/limit/*  reply got_limit, fail limit_failed"));
}

/// A local risk service: /limit/A answers {"value": 300}, anything else a 500.
fn risk_server() -> u16 {
    let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = srv.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for req in srv.incoming_requests() {
            let resp = if req.url() == "/limit/A" {
                tiny_http::Response::from_string(r#"{"value": 300}"#).with_status_code(200)
            } else {
                tiny_http::Response::from_string("no").with_status_code(500)
            };
            let _ = req.respond(resp);
        }
    });
    port
}

#[test]
fn real_http_reply_and_failure() {
    // one run against a real local server: a 200 sets the limit, a 500 becomes limit_failed
    let defs = limits();
    let reps = reports(&defs);
    let mut h = limits_host(&defs, &reps, None, risk_server());
    call(&mut h, "request_limit", &[s("A")]);
    call(&mut h, "request_limit", &[s("B")]);
    h.flush().unwrap();
    let results: IndexMap<String, String> =
        h.pump(vec![]).unwrap().into_iter().map(|(n, _, r)| (n, r["status"].as_str().unwrap().to_string())).collect();
    assert_eq!(results, IndexMap::from([("got_limit".to_string(), "ok".to_string()), ("limit_failed".into(), "ok".into())]));
    let w = h.load().unwrap();
    assert_eq!(w.maps["limit"][&Key::Str("A".into())], 300);
    assert_eq!(w.maps["pending"], IndexMap::from([(Key::Str("A".into()), 0), (Key::Str("B".into()), 0)]));
    assert_eq!(call(&mut h, "withdraw", &[s("A"), s("K"), i(200)])["status"], "ok");
    assert_eq!(call(&mut h, "withdraw", &[s("B"), s("K"), i(1)])["refused"], "OverLimit");
    assert!(h.replay().unwrap().is_empty());
}

#[test]
fn serve_runs_network_replies() {
    // kern-44's R2: under `serve`, a request's reply runs as a new call
    let defs = limits();
    let reps = reports(&defs);
    let mut h = limits_host(&defs, &reps, None, risk_server());
    let st = serve_requests(&mut h, 3, |base| {
        for a in ["A", "B"] {
            assert_eq!(post(&base, "request_limit", json!({"a": a})).0, 200);
        }
        get(&base, "/state")["state"].clone() // served after the replies ran
    });
    assert_eq!(st["limit"], json!({"A": 300}));
    assert_eq!(st["pending"], json!({"A": 0, "B": 0}));
    let names: Vec<String> =
        h.db.prepare("select name from log order by seq").unwrap().query_map([], |r| r.get(0)).unwrap().map(|x| x.unwrap()).collect();
    assert_eq!(names[names.len() - 4..], ["request_limit", "got_limit", "request_limit", "limit_failed"]);
    assert!(h.replay().unwrap().is_empty());
}

#[test]
fn repl_runs_network_replies_and_says_so() {
    let defs = limits();
    let reps = reports(&defs);
    let mut h = limits_host(&defs, &reps, None, risk_server());
    let mut out = vec![];
    let lines = ["request_limit A", "request_limit B", "withdraw A K 200"].iter().map(|l| l.to_string());
    kern::host::repl(&mut h, lines, &mut |l| out.push(l)).unwrap();
    assert_eq!(out, ["ok", "reply got_limit A 1 300 -> ok", "ok", "reply limit_failed B 1 -> ok", "ok"]);
}

#[test]
fn a_new_database_without_world_gets_an_empty_genesis() {
    let d = tmp("hostdb");
    let host_cli = |args: &[&str], stdin: &str| -> (i32, String) {
        use std::io::Write;
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_kern"))
            .arg("host")
            .args(args)
            .current_dir(ROOT)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let o = c.wait_with_output().unwrap();
        (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let db = d.join("x.db").to_string_lossy().into_owned();
    assert_eq!(host_cli(&["repl", "examples/shop", "--db", &db], "restock 1 2\n").1, "ok");
    assert_eq!(host_cli(&["replay", "examples/shop", "--db", &db], "").1, "replay of 1 logged calls matches the database");
    let db2 = d.join("y.db").to_string_lossy().into_owned();
    assert_eq!(host_cli(&["repl", "examples/shop", "--db", &db2, "--world", ""], "restock 1 2\n").0, 0);
    assert!(host_cli(&["replay", "examples/shop", "--db", &db2], "").1.contains("matches the database"));
}

// --- item 8: crash recovery (a real process, killed with SIGKILL during a flush)

/// The child: flush every undelivered event of the database in KERN_TEST_FLUSH, slowly, appending each seq to
/// KERN_TEST_LOG. Run by kill_during_flush... as this test binary re-executed with only this test selected.
#[test]
fn flush_child() {
    let (Ok(db), Ok(log)) = (std::env::var("KERN_TEST_FLUSH"), std::env::var("KERN_TEST_LOG")) else { return };
    let defs = escrow();
    let slow: Sink = Box::new(move |seq, _| {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&log).unwrap();
        writeln!(f, "{seq}").unwrap();
        f.sync_all().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    });
    let mut h = Host::new(&defs, IndexMap::new(), Options { db, sinks: Some(vec![slow]), gate: false, ..Default::default() }).unwrap();
    h.flush().unwrap();
}

#[test]
fn kill_during_flush_redelivers_with_at_most_one_duplicate_per_kill() {
    let d = tmp("hostkill");
    let (db, log) = (d.join("e.db").to_string_lossy().into_owned(), d.join("delivered.txt"));
    let defs = escrow();
    {
        let mut h = Host::new(&defs, reports(&defs), Options { db: db.clone(), clock: Some(Box::new(|| 1)), sinks: Some(vec![]), ..Default::default() }).unwrap();
        h.init(WORLD).unwrap();
        for _ in 0..40 {
            assert_eq!(call(&mut h, "transfer", &[s("A"), s("B"), i(1)])["status"], "ok"); // 40 committed, undelivered
        }
    }
    let lines = || std::fs::read_to_string(&log).map(|t| t.split_whitespace().map(|x| x.parse::<i64>().unwrap()).collect()).unwrap_or_else(|_| vec![]);
    let child = || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "flush_child", "--nocapture", "--test-threads", "1"])
            .env("KERN_TEST_FLUSH", &db)
            .env("KERN_TEST_LOG", &log)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut kills = 0;
    for stop_after in [5, 15, 27] {
        // kill -9 three times mid-flush
        let mut p = child();
        while lines().len() < stop_after && p.try_wait().unwrap().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        p.kill().unwrap(); // SIGKILL
        p.wait().unwrap();
        kills += 1;
    }
    assert!(child().wait().unwrap().success()); // restart, finish
    let got: Vec<i64> = lines();
    let uniq: std::collections::BTreeSet<i64> = got.iter().copied().collect();
    assert_eq!(uniq.into_iter().collect::<Vec<_>>(), (1..=40).collect::<Vec<_>>()); // every committed event delivered
    let dups = got.len() - got.iter().collect::<std::collections::BTreeSet<_>>().len();
    assert!(dups <= kills, "{dups} duplicates for {kills} kills");
    eprintln!("events 40, kills {kills}, deliveries {}, duplicates {dups}", got.len());
}

// --- H5: the proof page says what the host assumes, as assumed

#[test]
fn proof_page_host_section_is_assumed_not_proven() {
    let defs = limits();
    let reps = reports(&defs);
    let h = limits_host(&defs, &reps, Some(Box::new(SimNet::new(Box::new(|_| json!({"value": 1})), 0))), 1);
    let facts = host_facts(&h);
    let section = kern::render::host_html(Some(&facts));
    assert!(section.contains("assumed, not proven") && !section.contains('✔'));
    assert!(section.contains("GET <code>http://127.0.0.1:1/limit/{1}</code>") && section.contains("at least once"));
    assert_eq!(section.matches("badge warn'>assumed</span>").count(), facts.assumptions.len());
    let e = escrow();
    let plain_host = host(&e, WORLD, opts()); // escrow: no network
    let plain = kern::render::host_html(Some(&host_facts(&plain_host)));
    assert!(!plain.contains("Network endpoints") && !plain.contains("network reply"));
}

// --- S7 (docs/research/ROADMAP_RUN.md): proven checks removed at runtime

#[test]
fn runtime_checks_are_on_by_default_and_skip_proven_removes_proven_ones() {
    let defs = escrow();
    let h = host(&defs, WORLD, opts());
    assert_eq!(h.manifest()["transfer"]["runtime_invariant_check"], "on");
    assert!(manifest_text(&h).contains("on for every definition (the default"));
    let h = host(&defs, WORLD, Options { skip_proven: true, ..opts() });
    assert_eq!(h.manifest()["transfer"]["runtime_invariant_check"], "removed (proven)");
    assert!(manifest_text(&h).contains("--skip-proven") && manifest_text(&h).contains("sampled audit rate"));
    let burn = module(BURN);
    let mut b = host(&burn, "A:10@u1", Options { gate: false, skip_proven: true, ..opts() }); // unproven: still checked
    assert_eq!(b.manifest()["burn"]["runtime_invariant_check"], "on");
    assert!(call(&mut b, "burn", &[s("A"), i(50)])["error"].as_str().unwrap().contains("invariant"));
}

#[test]
fn the_default_catches_an_invariant_broken_outside_kern_and_skip_proven_does_not() {
    let defs = escrow();
    for (skip, want) in [(false, "error"), (true, "ok")] {
        let mut h = host(&defs, WORLD, Options { skip_proven: skip, ..opts() });
        h.db.execute("update account set balance = -5 where name='B'", []).unwrap(); // outside Kern
        let r = call(&mut h, "transfer", &[s("A"), s("B"), i(1)]); // B ends at -4
        assert_eq!(r["status"], want, "{skip} {r:?}");
    }
}

#[test]
fn startup_audit_refuses_a_database_broken_outside_kern() {
    let db = tmp("hostaudit").join("e.db").to_string_lossy().into_owned();
    let defs = escrow();
    {
        let mut h = Host::new(&defs, reports(&defs), Options { db: db.clone(), clock: Some(Box::new(|| 1)), sinks: Some(vec![]), ..Default::default() }).unwrap();
        h.init(WORLD).unwrap();
        h.db.execute("update account set balance = -5 where name='B'", []).unwrap(); // e.g. another process
    }
    let e = Host::new(&defs, reports(&defs), Options { db, sinks: Some(vec![]), ..Default::default() }).err().unwrap().0;
    assert!(e.contains("startup audit"), "{e}");
}

#[test]
fn sampled_audit_runs_skipped_checks() {
    let defs = escrow();
    let mut h = host(&defs, WORLD, Options { skip_proven: true, audit_rate: 1.0, ..opts() });
    for _ in 0..5 {
        call(&mut h, "transfer", &[s("A"), s("B"), i(1)]);
    }
    assert_eq!((h.audit_sampled, h.audit_violations), (5, 0));
}

#[test]
fn a_provider_with_no_value_fails_the_call_atomically() {
    // FZ finding 2 in the host: randbelow(0) must be a call error, not a crash
    let defs = module(
        "handler rand.below n:Int -> Int\n  ensure result >= 0 and result < n\n\nstate hold Int\n\npick n:Int\n  ensure hold.get' 0 >= 0\nbody\n  hold.set 0 9\n  r = rand.below n\n  hold.set 0 r\n",
    );
    let mut h = host(&defs, "A:1@u1", Options { gate: false, ..opts() });
    let r = call(&mut h, "pick", &[i(0)]);
    assert!(r["status"] == "error" && r["error"].as_str().unwrap().contains("gave no value"), "{r:?}");
    let w = h.load().unwrap();
    assert_eq!(w.maps.get("hold").and_then(|m| m.get(&Key::Int(0))).copied().unwrap_or(0), 0); // nothing written
}

#[test]
fn s5_a_constant_runs_as_its_value_in_the_host_and_replays() {
    // the host sees the substituted literal: same answers as the program with the values written in
    let src = std::fs::read_to_string(format!("{ROOT}/tests/probes/consts.k")).unwrap();
    let mut outs = vec![];
    for text in [src.clone(), common::inline_consts(&src)] {
        let defs = module(&text);
        let mut h = host(&defs, "A:500@u1", opts());
        let mut out = vec![];
        let lines = ["charge A 1 10", "charge A 1 3", "charge A 2 95", "charge A 1 120", "relay A 1 6", "bump 0", "state"];
        kern::host::repl(&mut h, lines.iter().map(|l| l.to_string()), &mut |l| out.push(l)).unwrap();
        assert!(h.replay().unwrap().is_empty());
        outs.push(out);
    }
    assert_eq!(outs[0], outs[1]);
    let o = &outs[0];
    assert_eq!(&o[..2], ["ok".to_string(), "refused TooSmall 5".to_string()], "{o:?}");
    assert!(o.contains(&"refused Over 95".to_string()), "{o:?}"); // lim - hold.1 = 100 - 5
}

#[test]
fn s5_a_record_parameter_is_one_json_object_over_http() {
    // 38ceceb: a record travels as one JSON object; the flattened keys, a missing or extra field, and the wrong
    // shape are 400s that never reach the log; a wrong field type is a bad call naming the field
    let defs = module(
        "state hold Int\nrecord Point x:Int y:Int\nrecord Pay from:Account to:Account amount:Int\n\n\
         put p:Point k:Int\n  need p.y >= 0 else Neg\n  ok otherwise\n  ensure hold.get' (p.x + k) = p.y\nbody\n  hold.set (p.x + k) p.y\n\n\
         send p:Pay\n  need p.amount > 0 else BadAmount\n  need p.from != p.to else Same\n  need ledger.balance p.from >= p.amount else Insufficient\n  ok otherwise\nbody\n  ledger.debit p.from p.amount\n  ledger.credit p.to p.amount\n",
    );
    let mut h = host(&defs, "A:100@u1 B:0@u2", opts());
    let out = serve_requests(&mut h, 10, |base| {
        [
            post(&base, "put", json!({"p": {"x": 1, "y": 2}, "k": 3})),
            post(&base, "put", json!({"p": {"y": 7, "x": 1}, "k": 0})),
            post(&base, "put", json!({"p.x": 1, "p.y": 2, "k": 3})),
            post(&base, "put", json!({"p": {"x": 1}, "k": 3})),
            post(&base, "put", json!({"p": {"x": 1, "y": 2, "z": 0}, "k": 3})),
            post(&base, "put", json!({"p": [1, 2], "k": 3})),
            post(&base, "put", json!({"p": {"x": 1, "y": -1}, "k": 3})),
            post(&base, "put", json!({"p": {"x": "1", "y": 2}, "k": 3})),
            post(&base, "send", json!({"p": {"from": "A", "to": "B", "amount": 10}})),
            post(&base, "send", json!({"p": {"from": "A", "to": "Z", "amount": 10}})),
        ]
    });
    let codes: Vec<u16> = out.iter().map(|o| o.0).collect();
    assert_eq!(codes, [200, 200, 400, 400, 400, 400, 409, 400, 200, 400], "{out:?}");
    assert!(out[7].1["error"].as_str().unwrap().contains("`p.x` must be an Int"), "{out:?}");
    assert!(out[9].1["error"].as_str().unwrap().contains("`p.to`"), "{out:?}");
    let mut st = vec![];
    kern::host::repl(&mut h, ["state".to_string()].into_iter(), &mut |l| st.push(l)).unwrap();
    let st: J = serde_json::from_str(&st[0]).unwrap();
    assert_eq!((st["balances"]["A"].clone(), st["state"]["hold"]["4"].clone(), st["state"]["hold"]["1"].clone()), (json!(90), json!(2), json!(7)));
    assert!(h.replay().unwrap().is_empty());
}

#[test]
fn s20_an_int_outside_i64_is_a_host_error_and_rolled_back() {
    // 8e2f1b5 (S20 host side): a stored balance, an event, a state value or a result outside i64 is an error and
    // the call is undone; an out-of-range or non-integer JSON number is a bad call; replay matches afterwards
    let defs = module(
        "state hold Int\n\ngive a:Account n:Int\n  need n > 0 else Bad\n  ok otherwise\nbody\n  ledger.credit a n\n\n\
         shout n:Int\n  ok otherwise\n  emit Big (n + n)\nbody\n  audit.emit (Big (n + n))\n\n\
         many n:Int\n  ok otherwise\nbody\n  hold.set 1 (sum j from 0 to 3 of n)\n\n\
         twice n:Int -> Int\n  ensure result = n + n\nbody\n  result = n + n\n",
    );
    let mut h = host(&defs, "A:9223372036854775000@u1 B:0@u2", opts());
    let lines = ["give A 1000", "give B 5", "shout 5000000000000000000", "shout 3", "many 4000000000000000000", "many 2",
                 "twice 5000000000000000000", "twice 4", "state"];
    let mut out = vec![];
    kern::host::repl(&mut h, lines.iter().map(|l| l.to_string()), &mut |l| out.push(l)).unwrap();
    let errors = out.iter().filter(|l| l.contains("out of the host's range (i64)")).count();
    assert_eq!(errors, 4, "{out:?}");
    let st: J = serde_json::from_str(out.last().unwrap()).unwrap();
    assert_eq!((st["balances"]["A"].clone(), st["balances"]["B"].clone(), st["state"]["hold"]["1"].clone()),
               (json!(9223372036854775000i64), json!(5), json!(6)));
    assert!(h.replay().unwrap().is_empty());
    let out = serve_requests(&mut h, 3, |base| {
        [post(&base, "give", json!({"a": "B", "n": 1.5})).0, post(&base, "give", json!({"a": "B", "n": 2.0})).0,
         post(&base, "give", json!({"a": "B", "n": 9223372036854775808u64})).0]
    });
    assert_eq!(out, [400, 400, 400]);
}

/// A network that never answers; replies are injected by hand.
struct Silent;
impl kern::host::Net for Silent {
    fn send(&mut self, _seq: i64, _ev: &Value, _eps: &IndexMap<String, kern::host::Endpoint>) -> Vec<(String, Vec<Value>)> {
        vec![]
    }
}

#[test]
fn s12_deadlines_late_replies_manual_answers_and_replay() {
    // S12 (f79f085): with `timeout 5` and one recorded clock second per call, an unanswered request gets its fail
    // call once its deadline passes, never before and never twice; a reply after that is Stale; a request answered
    // by an ordinary call before its deadline gets no timeout call; replay covers the timeout calls
    let defs = limits();
    let reps = reports(&defs);
    let eps = load_config(&format!("{CONFIG}; timeout 5").replace("{port}", "1"), &defs).unwrap();
    let mut h = Host::new(&defs, reps.clone(), Options { endpoints: Some(eps), net: Some(Box::new(Silent)), ..opts() }).unwrap();
    h.init(LIMITS_WORLD).unwrap();
    let timeouts: RefCell<Vec<(String, Vec<String>)>> = RefCell::new(vec![]);
    let mut step = |h: &mut Host, name: &str, args: &[Value], extra: Vec<(String, Vec<Value>)>| -> Vec<CallResult> {
        let r = call(h, name, args);
        h.flush().unwrap();
        let mut rs = vec![r];
        for (n, a, res) in h.pump(extra).unwrap() {
            if res.get("timeout").is_some() {
                timeouts.borrow_mut().push((n.clone(), a.iter().map(|v| v.str()).collect()));
            }
            rs.push(res);
        }
        rs
    };
    assert_eq!(step(&mut h, "request_limit", &[s("A")], vec![])[0]["status"], "ok"); // sent at t0
    assert_eq!(step(&mut h, "request_limit", &[s("B")], vec![])[0]["status"], "ok"); // sent at t0 + 1
    // B is answered by an ordinary call before its deadline
    assert_eq!(step(&mut h, "limit_failed", &[s("B"), i(1)], vec![])[0]["status"], "ok");
    for _ in 0..2 {
        step(&mut h, "transfer", &[s("A"), s("K"), i(1)], vec![]);
    }
    assert!(timeouts.borrow().is_empty(), "fired early: {:?}", timeouts.borrow());
    for _ in 0..6 {
        step(&mut h, "transfer", &[s("A"), s("K"), i(1)], vec![]);
    }
    let ts = timeouts.borrow().clone();
    assert_eq!(ts.len(), 1, "{ts:?}");
    assert_eq!((ts[0].0.as_str(), ts[0].1.clone()), ("limit_failed", vec!["A".to_string(), "1".to_string()]));
    // the late reply is refused, and A can ask again
    let late = step(&mut h, "transfer", &[s("A"), s("K"), i(1)], vec![("got_limit".into(), vec![s("A"), i(1), i(300)])]);
    assert_eq!(late[1]["refused"], "Stale", "{late:?}");
    assert_eq!(step(&mut h, "request_limit", &[s("A")], vec![])[0]["status"], "ok");
    assert!(h.replay().unwrap().is_empty());
    // a deadline is a non-negative number of seconds
    for bad in ["; timeout 0", "; timeout -1", "; timeout x", "; timeout"] {
        assert!(load_config(&format!("{CONFIG}{bad}").replace("{port}", "1"), &defs).is_err(), "{bad} accepted");
    }
}
