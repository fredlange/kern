//! A host for Kern modules (port of kern/host.py): the boundary between verified code and the operating system.
//!
//! Kern code never touches the OS. The host does, and keeps the three properties the proofs rest on (docs/HOST.md):
//!
//!   atomicity    one SQLite transaction per top-level call; a refusal or a runtime error writes no state and
//!                emits nothing
//!   closed world the module can only reach the handlers in run::HANDLERS; the host maps them:
//!                ledger + `state` maps -> SQLite tables (owned state, rolled back)
//!                clock.now             -> wall clock, read once per call and recorded (an observation)
//!                audit.emit            -> an outbox table, delivered to sinks after commit (an action)
//!   determinism  every attempted call is logged with its recorded `now`; `replay` re-runs the log from the
//!                genesis world and must reach the same state and outbox
//!
//! Delivery is at-least-once: a crash between delivering an outbox event and marking it delivered sends it again.
//! Consumers need the event's `seq` as an idempotency key.
//!
//!   kern host serve    FILE.k --db FILE.db [--world "A:500@u1 ..."] [--port 8080] [--bind ADDR]
//!   kern host repl     FILE.k --db FILE.db [--world ...]     (calls on stdin, outbox on stdout)
//!   kern host replay   FILE.k --db FILE.db
//!   kern host manifest FILE.k [--db FILE.db]
//!   kern host page     FILE.k [--html OUT.html]
//!
//! The database is the Python host's, row for row (same schema, same JSON in every text column), so a database
//! either host wrote can be served and replayed by the other.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::rc::Rc;

use indexmap::IndexMap;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value as J};

use crate::infer::{fails, uses};
use crate::pyjson;
use crate::pyrandom::Random;
use crate::pyrepr::repr_str;
use crate::render::HostInfo;
use crate::report::{Report, Status as GStatus};
use crate::run::{Interp, Outcome, ProvideError, Provider, RunError, Store, Value, World};
use crate::syntax::{flat, Emit, Expr, Key, Module, Stmt};
use crate::Int;

pub const SCHEMA: &str = "
create table if not exists meta(k text primary key, v text not null);
create table if not exists account(name text primary key, balance integer not null, owner text not null);
create table if not exists cell(state text not null, key text not null, value integer not null,
                                primary key(state, key));
create table if not exists textcell(state text not null, key text not null, value text not null,
                                    primary key(state, key));
create table if not exists log(seq integer primary key, name text not null, args text not null,
                               now integer not null, outcome text not null, observed text not null default '[]');
create table if not exists outbox(seq integer primary key, call integer not null, event text not null,
                                  delivered integer not null default 0, channel text not null default 'audit',
                                  answered integer not null default 0);
";

/// What can go wrong around a call (inside one, everything is the call's result).
#[derive(Debug)]
pub enum HostError {
    /// no definition with this name (Python's KeyError)
    NoDef(String),
    /// the gate: not proven clean
    Unproven(String),
    /// a host fault; the call rolled back (Python: any other exception)
    Fault(String, String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            HostError::NoDef(n) => f.write_str(&repr_str(n)),
            HostError::Unproven(n) => f.write_str(n),
            HostError::Fault(t, m) => write!(f, "{}: {}", t, m),
        }
    }
}

fn sql(e: rusqlite::Error) -> HostError {
    HostError::Fault("sqlite3.Error".into(), e.to_string())
}

/// The host config doesn't match the module; raised at startup, never during a call.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigError(pub String);

// ---------------------------------------------------------------- values as JSON (Python's json.dumps of them)

pub fn value_json(v: &Value) -> J {
    match v {
        Value::Int(n) => json!(*n as i64),
        Value::Bool(b) => json!(b),
        Value::Str(s) => json!(s),
        Value::None => J::Null,
        Value::Ctor(c, a) => {
            let mut xs = vec![json!(c)];
            xs.extend(a.iter().map(value_json));
            J::Array(xs)
        }
    }
}

/// JSON gives lists back; events and constructor values are tuples in the interpreter (Python's `_tuplify`).
pub fn json_value(j: &J) -> Value {
    match j {
        J::Null => Value::None,
        J::Bool(b) => Value::Bool(*b),
        // an Int is an i64 at the host (S20); anything else (1.5, 2^63) stays text, which no Int parameter accepts
        J::Number(n) => n.as_i64().map(|x| Value::Int(x as Int)).unwrap_or_else(|| Value::Str(number_text(n))),
        J::String(s) => Value::Str(s.clone()),
        J::Array(xs) => match xs.split_first() {
            Some((J::String(c), rest)) => Value::Ctor(c.clone(), rest.iter().map(json_value).collect()),
            _ => Value::Ctor(String::new(), xs.iter().map(json_value).collect()),
        },
        J::Object(_) => Value::None,
    }
}

/// Loop and sum iterations one call may start (S21): the interpreter's budget, charged `hi - lo` when a loop or sum
/// begins. A call past it is a host fault, so one huge bound can't hold the write lock. kern-rt's ITERATION_LIMIT is
/// the same number (emit_rust::tests checks).
pub const ITERATION_LIMIT: Int = 10_000_000;

/// The host's one Int range (S20): i64, what SQLite stores and the compiled runtime computes in. The interpreter
/// computes in i128; a call whose result leaves this range is a host fault, never a silently wrapped value.
pub fn fits(n: Int) -> bool {
    n >= i64::MIN as Int && n <= i64::MAX as Int
}

fn value_fits(v: &Value) -> bool {
    match v {
        Value::Int(n) => fits(*n),
        Value::Ctor(_, a) => a.iter().all(value_fits),
        _ => true,
    }
}

/// The first Int a successful call would store, emit or return that leaves the host's range.
fn out_of_range(world: &World, result: Option<&Value>) -> Option<String> {
    if let Some((a, b)) = world.bal.iter().find(|(_, b)| !fits(**b)) {
        return Some(format!("balance of {} = {}", a, b));
    }
    for (x, m) in &world.maps {
        for (k, v) in m {
            if matches!(k, Key::Int(n) if !fits(*n)) || !fits(*v) {
                return Some(format!("{}.{} = {}", x, key_str(k), v));
            }
        }
    }
    if let Some(e) = world.audit.iter().chain(world.out.values().flatten()).find(|e| !value_fits(e)) {
        return Some(format!("event {}", e.repr()));
    }
    match result {
        Some(r) if !value_fits(r) => Some(format!("result {}", r.repr())),
        _ => None,
    }
}

/// How a JSON number that isn't an i64 reads in an error: exact when serde kept it (a u64), else as parsed; an integer
/// past i64 says so (serde holds it as a float, so `-9223372036854775809` would otherwise echo as `-9.2…e18`).
fn number_text(n: &serde_json::Number) -> String {
    match (n.as_u64(), n.as_f64()) {
        (Some(u), _) => format!("{} (outside i64)", u),
        (None, Some(x)) if x.fract() == 0.0 => format!("{:e} (outside i64)", x),
        _ => n.to_string(),
    }
}

fn dumps(j: &J) -> String {
    pyjson::dumps(j, None)
}

fn key_json(k: &Key) -> String {
    match k {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => pyjson::string(s),
    }
}

fn key_from_json(text: &str) -> Key {
    match serde_json::from_str::<J>(text) {
        Ok(J::Number(n)) => Key::Int(n.as_i64().unwrap_or(0) as Int),
        Ok(J::String(s)) => Key::Str(s),
        _ => Key::Str(text.to_string()),
    }
}

/// Python's str() of an event, for the sinks: the constructor and its arguments, nested events in parentheses.
pub fn event_text(ev: &Value) -> String {
    match ev {
        Value::Ctor(c, a) => {
            let mut parts = vec![c.clone()];
            parts.extend(a.iter().map(|x| match x {
                Value::Ctor(..) => format!("({})", event_text(x)),
                other => other.str(),
            }));
            parts.join(" ")
        }
        other => other.str(),
    }
}

pub type Sink = Box<dyn FnMut(i64, &Value)>;

pub fn console_sink() -> Sink {
    Box::new(|seq, ev| println!("audit #{}: {}", seq, event_text(ev)))
}

/// The `console` channel: one line per event, the event's text without the constructor.
pub fn stdout_sink() -> Sink {
    Box::new(|_, ev| match ev {
        Value::Ctor(_, a) => println!(
            "{}",
            a.iter().map(|x| if matches!(x, Value::Ctor(..)) { event_text(x) } else { x.str() }).collect::<Vec<_>>().join(" ")
        ),
        other => println!("{}", other.str()),
    })
}

/// A call's events as (channel, event): audit first, then channels in declaration order. The order across
/// channels is a host convention; specs fix only the order within one channel.
fn ordered_events(world: &World, defs: &Module) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = world.audit.iter().map(|e| ("audit".to_string(), e.clone())).collect();
    for c in &defs.channels {
        for e in world.out.get(c).into_iter().flatten() {
            out.push((c.clone(), e.clone()));
        }
    }
    out
}

fn dict_repr<K, V>(items: impl IntoIterator<Item = (K, V)>, k: impl Fn(&K) -> String, v: impl Fn(&V) -> String) -> String {
    let parts: Vec<String> = items.into_iter().map(|(a, b)| format!("{}: {}", k(&a), v(&b))).collect();
    format!("{{{}}}", parts.join(", "))
}

fn key_repr(k: &Key) -> String {
    match k {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => repr_str(s),
    }
}

fn key_str(k: &Key) -> String {
    match k {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => s.clone(),
    }
}

// ---------------------------------------------------------------- the network

/// A path or query value: unreserved characters as they are, everything else %-encoded (a Text may hold anything;
/// account names and Ints are unchanged).
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Constructor -> argument types, for every event a module can send on `net` (bodies and specs).
pub fn net_requests(defs: &Module) -> IndexMap<String, Vec<String>> {
    let mut out: IndexMap<String, Vec<String>> = IndexMap::new();
    for d in defs.defs.values() {
        let ty = |a: &Expr| match a {
            Expr::Name(n, _) => d.params.iter().find(|(p, _)| p == n).map(|(_, t)| t.clone()).unwrap_or("Int".into()),
            Expr::Text(_) => "Text".into(),
            _ => "Int".into(),
        };
        for s in flat(&d.body) {
            if let Stmt::Do(Expr::App(h, _, args)) = s {
                if h == "net.emit" {
                    let (ctor, cargs): (String, &[Expr]) = match &args[0] {
                        Expr::App(c, _, a) => (c.clone(), a),
                        Expr::Name(c, _) => (c.clone(), &[]),
                        _ => continue,
                    };
                    out.entry(ctor).or_insert_with(|| cargs.iter().map(ty).collect());
                }
            }
        }
        for (i, ev) in d.emits.iter().enumerate() {
            if d.emit_channel.get(&i).map(|c| c.as_str()) == Some("net") {
                let (Emit::Ev(ctor, args, _) | Emit::As(ctor, args, _, _)) = ev;
                out.entry(ctor.clone()).or_insert_with(|| args.iter().map(ty).collect());
            }
        }
    }
    out
}

/// A request's deadline when its `.host` line has no `timeout` clause: seconds of the host clock.
pub const DEFAULT_TIMEOUT: Int = 300;

/// `Ctor -> GET URL; reply DEF ARGS; fail DEF ARGS[; timeout N]`. ARGS are `{i}` (the event's i-th argument) or
/// `$.field` (an Int field of the JSON reply; `reply` only). Kern never sees the bytes. A request no call has answered
/// within `timeout` seconds of the host clock gets its `fail` call (S12).
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub ctor: String,
    pub method: String,
    pub url: String,
    pub reply: String,
    pub reply_args: Vec<String>,
    pub fail: String,
    pub fail_args: Vec<String>,
    /// seconds of the host clock (DEFAULT_TIMEOUT when the line has no clause)
    pub timeout: Int,
    /// the reply definition's parameter types (load_config fills them): a `$.field` for a Text parameter takes a
    /// JSON string, for an Int one a JSON integer
    pub reply_types: Vec<String>,
    pub src: String,
}

impl Endpoint {
    pub fn parse(line: &str, no: usize) -> Result<Endpoint, ConfigError> {
        let bad =
            || ConfigError(format!("line {}: want `Ctor -> GET URL; reply DEF ARGS; fail DEF ARGS[; timeout SECONDS]`", no));
        let parts: Vec<&str> = line.split(';').map(str::trim).collect();
        let (head, reply, fail, timeout) = match parts[..] {
            [h, r, f] => (h, r, f, DEFAULT_TIMEOUT),
            [h, r, f, t] => match t.split_whitespace().collect::<Vec<_>>()[..] {
                ["timeout", n] => match n.parse::<Int>() {
                    Ok(n) if n > 0 => (h, r, f, n),
                    _ => return Err(bad()),
                },
                _ => return Err(bad()),
            },
            _ => return Err(bad()),
        };
        let hp: Vec<&str> = head.split("->").map(str::trim).collect();
        let [ctor, target] = hp[..] else { return Err(bad()) };
        let tp: Vec<&str> = target.split_whitespace().collect();
        let [method, url] = tp[..] else { return Err(bad()) };
        let rp: Vec<&str> = reply.split_whitespace().collect();
        let fp: Vec<&str> = fail.split_whitespace().collect();
        if rp.len() < 2 || fp.len() < 2 || rp[0] != "reply" || fp[0] != "fail" || !(method == "GET" || method == "POST") {
            return Err(bad());
        }
        Ok(Endpoint {
            ctor: ctor.into(),
            method: method.into(),
            url: url.into(),
            reply: rp[1].into(),
            reply_args: rp[2..].iter().map(|s| s.to_string()).collect(),
            fail: fp[1].into(),
            fail_args: fp[2..].iter().map(|s| s.to_string()).collect(),
            timeout,
            reply_types: vec![],
            src: line.trim().into(),
        })
    }

    /// The reply (or fail) call's arguments; Err when the body doesn't decode (that is a failure).
    pub fn args_for(&self, ev: &Value, body: Option<&J>, which: &str) -> Result<Vec<Value>, String> {
        let Value::Ctor(_, evargs) = ev else { return Err("not an event".into()) };
        let mut out = vec![];
        for (j, a) in (if which == "reply" { &self.reply_args } else { &self.fail_args }).iter().enumerate() {
            let text = which == "reply" && self.reply_types.get(j).map(|t| t == "Text").unwrap_or(false);
            if a.starts_with('{') {
                let i: usize = a[1..a.len() - 1].parse().map_err(|_| format!("bad argument {}", a))?;
                // ev[i]: index 0 is the constructor
                let v = if i == 0 { Value::Str(match ev { Value::Ctor(c, _) => c.clone(), _ => String::new() }) } else {
                    evargs.get(i - 1).cloned().ok_or_else(|| "tuple index out of range".to_string())?
                };
                out.push(v);
            } else {
                let mut v = body.ok_or_else(|| "'NoneType' object is not subscriptable".to_string())?;
                for k in a[2..].split('.') {
                    v = v.get(k).ok_or_else(|| repr_str(k))?;
                }
                match v {
                    J::String(t) if text => out.push(Value::Str(t.clone())),
                    J::Number(n) if !text && n.as_i64().is_some() => out.push(Value::Int(n.as_i64().unwrap() as Int)),
                    _ => return Err(format!("{} is not {}", a, if text { "a Text" } else { "an Int" })),
                }
            }
        }
        Ok(out)
    }

    /// Does the call `name args` answer the request `ev`: is it this endpoint's reply or fail definition, with the
    /// event's arguments wherever the config passes `{i}`?
    pub fn answers(&self, name: &str, args: &[Value], ev: &Value) -> bool {
        let Value::Ctor(c, evargs) = ev else { return false };
        [(&self.reply, &self.reply_args), (&self.fail, &self.fail_args)].iter().any(|(def, spec)| {
            *def == name
                && spec.len() == args.len()
                && spec.iter().zip(args).all(|(a, v)| match a.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                    Some(i) => match i.parse::<usize>() {
                        Ok(0) => *v == Value::Str(c.clone()),
                        Ok(i) => evargs.get(i - 1) == Some(v),
                        Err(_) => false,
                    },
                    None => true, // `$.field`: from the reply body, any value
                })
        })
    }

    pub fn url_for(&self, ev: &Value) -> String {
        let mut url = self.url.clone();
        if let Value::Ctor(_, args) = ev {
            for (i, a) in args.iter().enumerate() {
                url = url.replace(&format!("{{{}}}", i + 1), &url_encode(&a.str()));
            }
        }
        url
    }
}

/// Parse and validate a `.host` file against the module: every request constructor the module can send needs
/// exactly one endpoint, and reply/fail definitions must exist with matching types.
pub fn load_config(text: &str, defs: &Module) -> Result<IndexMap<String, Endpoint>, ConfigError> {
    let mut eps: IndexMap<String, Endpoint> = IndexMap::new();
    for (i, line) in crate::syntax::splitlines(text).iter().enumerate() {
        if !line.trim().is_empty() && !line.trim().starts_with('#') {
            let ep = Endpoint::parse(line, i + 1)?;
            eps.insert(ep.ctor.clone(), ep);
        }
    }
    let reqs = net_requests(defs);
    let mut problems = vec![];
    for ctor in reqs.keys() {
        if !eps.contains_key(ctor) {
            problems.push(format!("request {} has no endpoint in the host config", ctor));
        }
    }
    let list = |xs: &[String]| format!("[{}]", xs.iter().map(|x| repr_str(x)).collect::<Vec<_>>().join(", "));
    for (ctor, ep) in &eps {
        let Some(req) = reqs.get(ctor) else {
            problems.push(format!("endpoint for {}, which the module never sends", ctor));
            continue;
        };
        for (which, name, args) in [("reply", &ep.reply, &ep.reply_args), ("fail", &ep.fail, &ep.fail_args)] {
            for a in args.iter() {
                if which == "fail" && !a.starts_with('{') {
                    // the fail call also runs at the deadline (S12), when there is no reply at all
                    problems.push(format!("{}: fail {} takes {}, but a failure has no reply body", ctor, name, a));
                }
                let i = a.strip_prefix('{').and_then(|x| x.strip_suffix('}')).map(|x| x.parse::<usize>().ok());
                if matches!(i, Some(None)) || matches!(i, Some(Some(i)) if i > req.len()) {
                    problems.push(format!("{}: {} {} takes {}, and {} has {} arguments", ctor, which, name, a, ctor, req.len()));
                }
            }
            let Some(d) = defs.defs.get(name) else {
                problems.push(format!("{}: {} definition {} does not exist", ctor, which, repr_str(name)));
                continue;
            };
            let want: Vec<String> = d.params.iter().map(|(_, t)| t.clone()).collect();
            let got: Vec<String> = args
                .iter()
                .enumerate()
                .map(|(j, a)| {
                    if a.starts_with('{') {
                        let i: usize = a[1..a.len() - 1].parse().unwrap_or(0);
                        req.get(i.wrapping_sub(1)).cloned().unwrap_or_else(|| "Int".into())
                    } else {
                        // a reply field decodes to what the definition takes there, an Int or a Text
                        match want.get(j).map(String::as_str) {
                            Some("Text") => "Text".into(),
                            _ => "Int".into(),
                        }
                    }
                })
                .collect();
            if got != want {
                problems.push(format!("{}: {} {} takes {}, the config gives {}", ctor, which, name, list(&want), list(&got)));
            }
        }
    }
    if !problems.is_empty() {
        return Err(ConfigError(problems.join("; ")));
    }
    for ep in eps.values_mut() {
        ep.reply_types = defs.defs[&ep.reply].params.iter().map(|(_, t)| t.clone()).collect();
    }
    Ok(eps)
}

/// Where `net` requests go: the reply calls each request produces (now or never).
pub trait Net {
    fn send(&mut self, seq: i64, ev: &Value, endpoints: &IndexMap<String, Endpoint>) -> Vec<(String, Vec<Value>)>;
    /// replies held back (a simulated network); a real network has none
    fn release(&mut self) -> Vec<(String, Vec<Value>)> {
        vec![]
    }
}

/// Real requests. A timeout, a non-2xx status or a decode error becomes the `fail` call.
pub struct HttpNet {
    pub timeout: std::time::Duration,
}

impl Net for HttpNet {
    fn send(&mut self, _seq: i64, ev: &Value, endpoints: &IndexMap<String, Endpoint>) -> Vec<(String, Vec<Value>)> {
        let Value::Ctor(c, _) = ev else { return vec![] };
        let Some(ep) = endpoints.get(c) else { return vec![] };
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();
        let reply = agent
            .request(&ep.method, &ep.url_for(ev))
            .call()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_json::<J>().map_err(|e| e.to_string()))
            .and_then(|body| ep.args_for(ev, Some(&body), "reply"));
        match reply {
            Ok(args) => vec![(ep.reply.clone(), args)],
            Err(_) => vec![(ep.fail.clone(), ep.args_for(ev, None, "fail").unwrap_or_default())],
        }
    }
}

/// A hostile network for tests, seeded: each request's reply may be dropped, duplicated, delayed (held back and
/// released later, out of order) or failed. `answer(ev)` gives the JSON body a well-behaved server would send.
pub struct SimNet {
    pub answer: Box<dyn FnMut(&Value) -> J>,
    pub rng: Random,
    pub p: (f64, f64, f64, f64),
    pub held: Vec<(String, Vec<Value>)>,
}

impl SimNet {
    pub fn new(answer: Box<dyn FnMut(&Value) -> J>, seed: i128) -> SimNet {
        SimNet { answer, rng: Random::from_int(seed), p: (0.15, 0.2, 0.3, 0.1), held: vec![] }
    }
}

impl Net for SimNet {
    fn send(&mut self, _seq: i64, ev: &Value, endpoints: &IndexMap<String, Endpoint>) -> Vec<(String, Vec<Value>)> {
        let Value::Ctor(c, _) = ev else { return vec![] };
        let ep = &endpoints[c];
        let (p_drop, p_dup, p_delay, p_fail) = self.p;
        if self.rng.random() < p_drop {
            return vec![];
        }
        let failed = self.rng.random() < p_fail;
        let fail_call = || (ep.fail.clone(), ep.args_for(ev, None, "fail").unwrap_or_default());
        let call = if failed {
            fail_call()
        } else {
            let body = (self.answer)(ev);
            match ep.args_for(ev, Some(&body), "reply") {
                Ok(a) => (ep.reply.clone(), a),
                Err(_) => fail_call(), // an undecodable reply is a failure
            }
        };
        let calls = if self.rng.random() < p_dup { vec![call.clone(), call] } else { vec![call] };
        if self.rng.random() < p_delay {
            self.held.extend(calls);
            return vec![];
        }
        calls
    }

    /// Deliver some held-back replies, in a random order.
    fn release(&mut self) -> Vec<(String, Vec<Value>)> {
        self.rng.shuffle(&mut self.held);
        let k = self.rng.randint(0, self.held.len() as i64) as usize;
        let rest = self.held.split_off(k);
        std::mem::replace(&mut self.held, rest)
    }
}

// ---------------------------------------------------------------- the SQLite store behind a call's world

struct SqliteStore {
    db: Rc<Connection>,
}

impl Store for SqliteStore {
    fn balance(&self, a: &str) -> Option<Int> {
        self.db
            .query_row("select balance from account where name=?", [a], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .map(|x| x as Int)
    }
    fn owner(&self, a: &str) -> Option<String> {
        self.db.query_row("select owner from account where name=?", [a], |r| r.get(0)).optional().ok().flatten()
    }
    fn total(&self) -> Int {
        self.db.query_row("select coalesce(sum(balance), 0) from account", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as Int
    }
    fn text_cell(&self, state: &str, key: &Key) -> Option<String> {
        self.db
            .query_row("select value from textcell where state=? and key=?", params![state, key_json(key)], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }
    fn cell(&self, state: &str, key: &Key) -> Option<Int> {
        self.db
            .query_row("select value from cell where state=? and key=?", params![state, key_json(key)], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .map(|x| x as Int)
    }
}

pub type ProviderFn = Box<dyn FnMut(&[Value]) -> Result<Value, ProvideError>>;

/// The OS random source (Python: secrets.randbelow).
pub fn system_below() -> ProviderFn {
    Box::new(|args| {
        let n = args.first().and_then(|a| a.num()).unwrap_or(0);
        if n <= 0 {
            return Err(ProvideError::Other("ValueError".into(), "Upper bound must be positive.".into()));
        }
        let n = n as u128;
        let bits = 128 - n.leading_zeros();
        let mut f = std::fs::File::open("/dev/urandom").map_err(|e| ProvideError::Kern(e.to_string()))?;
        loop {
            let mut b = [0u8; 16];
            std::io::Read::read_exact(&mut f, &mut b).map_err(|e| ProvideError::Kern(e.to_string()))?;
            let x = u128::from_le_bytes(b) >> (128 - bits);
            if x < n {
                return Ok(Value::Int(x as Int));
            }
        }
    })
}

pub fn default_providers() -> IndexMap<String, ProviderFn> {
    IndexMap::from([("rand.below".to_string(), system_below())])
}

/// How a host is set up; Host::new's keyword arguments in Python.
pub struct Options {
    pub db: String,
    pub clock: Option<Box<dyn FnMut() -> Int>>,
    pub sinks: Option<Vec<Sink>>,
    pub channel_sinks: Option<IndexMap<String, Vec<Sink>>>,
    pub gate: bool,
    pub net: Option<Box<dyn Net>>,
    pub endpoints: Option<IndexMap<String, Endpoint>>,
    pub providers: Option<IndexMap<String, ProviderFn>>,
    pub skip_proven: bool,
    pub audit_rate: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            db: ":memory:".into(),
            clock: None,
            sinks: None,
            channel_sinks: None,
            gate: true,
            net: None,
            endpoints: None,
            providers: None,
            skip_proven: false,
            audit_rate: 0.0,
        }
    }
}

/// A call's result: Python's `res` dict, in its key order.
pub type CallResult = Map<String, J>;

pub struct Host<'m> {
    pub defs: &'m Module,
    providers: IndexMap<String, ProviderFn>,
    clock: Box<dyn FnMut() -> Int>,
    pub sinks: Vec<Sink>,
    pub channel_sinks: IndexMap<String, Vec<Sink>>,
    pub db: Rc<Connection>,
    pub reports: IndexMap<String, Report>,
    pub gate: bool,
    pub skip_proven: bool,
    invariant_proven: IndexMap<String, bool>,
    pub audit_rate: f64,
    pub audit_sampled: u64,
    pub audit_violations: u64,
    audit_rng: Random,
    pub endpoints: IndexMap<String, Endpoint>,
    pub net: Option<Box<dyn Net>>,
    /// replies waiting to become top-level calls
    pub inbox: Vec<(String, Vec<Value>)>,
    /// the earliest `now` the next call may have: one past where the last call that used clock.tick ended (meta
    /// `clock_floor`), so time never goes back across calls; 0 until a call ticks
    clock_floor: Int,
}

fn invariants_proven(r: &Report) -> bool {
    let goals: Vec<_> = r
        .guarantees
        .iter()
        .filter(|g| g.kind == "invariant" || (g.kind == "loop" && g.clause.starts_with("module invariant")))
        .collect();
    !goals.is_empty() && goals.iter().all(|g| g.status == GStatus::Proven)
}

impl<'m> Host<'m> {
    /// `reports` = the module's analysis (Python computes analyse(defs, mutation=False) when none is given).
    pub fn new(defs: &'m Module, reports: IndexMap<String, Report>, opts: Options) -> Result<Host<'m>, ConfigError> {
        let providers = opts.providers.unwrap_or_else(default_providers);
        let missing: BTreeSet<&String> = defs.handlers.keys().filter(|h| !providers.contains_key(*h)).collect();
        if !missing.is_empty() {
            return Err(ConfigError(format!(
                "the module declares {}, which this host does not provide",
                missing.into_iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let db = Connection::open(&opts.db).map_err(|e| ConfigError(format!("{}: {}", opts.db, e)))?;
        let cfg = |e: rusqlite::Error| ConfigError(e.to_string());
        db.execute_batch(SCHEMA).map_err(cfg)?;
        let cols = |t: &str| -> Result<Vec<String>, rusqlite::Error> {
            let mut st = db.prepare(&format!("pragma table_info({})", t))?;
            let names = st.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
            Ok(names)
        };
        if !cols("log").map_err(cfg)?.iter().any(|c| c == "observed") {
            db.execute_batch("alter table log add column observed text not null default '[]'").map_err(cfg)?;
        }
        if !cols("outbox").map_err(cfg)?.iter().any(|c| c == "channel") {
            // a database from before channels
            db.execute_batch("alter table outbox add column channel text not null default 'audit'").map_err(cfg)?;
        }
        if !cols("outbox").map_err(cfg)?.iter().any(|c| c == "answered") {
            // a database from before request deadlines: its requests are not timed out after the fact
            db.execute_batch(
                "begin; alter table outbox add column answered integer not null default 0; update outbox set answered=-1; commit",
            )
            .map_err(cfg)?;
        }
        let invariant_proven = reports.iter().map(|(n, r)| (n.clone(), invariants_proven(r))).collect();
        let mut h = Host {
            defs,
            providers,
            clock: opts.clock.unwrap_or_else(|| {
                Box::new(|| {
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as Int).unwrap_or(0)
                })
            }),
            sinks: opts.sinks.unwrap_or_else(|| vec![console_sink()]),
            channel_sinks: opts.channel_sinks.unwrap_or_else(|| IndexMap::from([("console".to_string(), vec![stdout_sink()])])),
            db: Rc::new(db),
            reports,
            gate: opts.gate,
            skip_proven: opts.skip_proven,
            invariant_proven,
            audit_rate: opts.audit_rate,
            audit_sampled: 0,
            audit_violations: 0,
            audit_rng: Random::from_int(0),
            endpoints: IndexMap::new(),
            net: None,
            inbox: vec![],
            clock_floor: 0,
        };
        // the induction also has to start right: a database from elsewhere is audited in full before serving
        if h.has_genesis().map_err(|e| ConfigError(e.to_string()))? {
            let mut w = h.load().map_err(|e| ConfigError(e.to_string()))?;
            let broken = Interp::new(defs).broken_invariants(&mut w, None).map_err(|e| ConfigError(e.to_string()))?;
            if let Some((text, key)) = broken.first() {
                return Err(ConfigError(format!(
                    "startup audit: invariant {} is broken at {}; not serving",
                    text,
                    key.str()
                )));
            }
        }
        // the network is policy: every request the module can send needs a configured endpoint, checked now
        let reqs = net_requests(defs);
        if !reqs.is_empty() && opts.endpoints.is_none() {
            let mut names: Vec<&String> = reqs.keys().collect();
            names.sort();
            return Err(ConfigError(format!(
                "the module sends {} on net, and there is no host config",
                names.into_iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        h.clock_floor = h
            .db
            .query_row("select v from meta where k='clock_floor'", [], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| ConfigError(e.to_string()))?
            .and_then(|v| v.parse::<i64>().ok())
            .map_or(0, |v| v as Int);
        h.endpoints = opts.endpoints.unwrap_or_default();
        h.net = opts.net;
        if !reqs.is_empty() && h.net.is_none() {
            h.net = Some(Box::new(HttpNet { timeout: std::time::Duration::from_secs(2) }));
        }
        Ok(h)
    }

    pub fn runtime_check_skipped(&self, name: &str) -> bool {
        self.skip_proven && self.invariant_proven.get(name).copied().unwrap_or(false)
    }

    fn has_genesis(&self) -> Result<bool, rusqlite::Error> {
        Ok(self.db.query_row("select 1 from meta where k='genesis'", [], |_| Ok(())).optional()?.is_some())
    }

    // --- the world in SQLite

    /// Create the genesis world once. Ok(false) if the database already has one.
    pub fn init(&mut self, world_src: &str) -> Result<bool, String> {
        if self.has_genesis().map_err(|e| e.to_string())? {
            return Ok(false);
        }
        // the eg world grammar: accounts, `state.key:N`, Text entries `state.key:"…"` (quoted)
        let ex = crate::syntax::parse_call(world_src, "genesis").map_err(|e| e.0)?;
        let mut w = World::from_example(&ex.world, 0, &ex.maps).with_texts(&ex.texts);
        if let Some(what) = out_of_range(&w, None) {
            return Err(format!("genesis world: Int out of the host's range (i64): {}", what));
        }
        let broken = Interp::new(self.defs).broken_invariants(&mut w, None).map_err(|e| e.to_string())?;
        if let Some((text, key)) = broken.first() {
            return Err(format!("genesis world breaks invariant {} at {}", text, key.str()));
        }
        let run = || -> Result<(), rusqlite::Error> {
            self.db.execute_batch("begin immediate")?;
            self.db.execute("insert into meta values('genesis', ?)", [dumps(&Self::dump(&w))])?;
            self.write(&World::default(), &w)?;
            self.db.execute_batch("commit")
        };
        if let Err(e) = run() {
            let _ = self.db.execute_batch("rollback");
            return Err(e.to_string());
        }
        Ok(true)
    }

    fn dump(w: &World) -> J {
        let mut d = Self::dump_ints(w);
        if w.texts.values().any(|m| !m.is_empty()) {
            // only when there is Text state, so a genesis without it is stored exactly as before
            d["texts"] = w
                .texts
                .iter()
                .map(|(s, m)| {
                    (s.clone(), J::Array(m.iter().map(|(k, v)| json!([match k { Key::Int(n) => json!(*n as i64), Key::Str(s) => json!(s) }, v])).collect()))
                })
                .collect::<Map<String, J>>()
                .into();
        }
        d
    }

    fn dump_ints(w: &World) -> J {
        json!({
            "bal": w.bal.iter().map(|(a, b)| (a.clone(), json!(*b as i64))).collect::<Map<String, J>>(),
            "own": w.own.iter().map(|(a, o)| (a.clone(), json!(o))).collect::<Map<String, J>>(),
            "maps": w.maps.iter().map(|(s, m)| (s.clone(), J::Array(m.iter().map(|(k, v)| json!([
                match k { Key::Int(n) => json!(*n as i64), Key::Str(s) => json!(s) }, *v as i64])).collect()))).collect::<Map<String, J>>(),
        })
    }

    fn undump(d: &J) -> World {
        let mut w = World::default();
        for (a, b) in d["bal"].as_object().into_iter().flatten() {
            w.bal.insert(a.clone(), b.as_i64().unwrap_or(0) as Int);
        }
        for (a, o) in d["own"].as_object().into_iter().flatten() {
            w.own.insert(a.clone(), o.as_str().unwrap_or("").to_string());
        }
        for (s, kv) in d["maps"].as_object().into_iter().flatten() {
            let m = w.maps.entry(s.clone()).or_default();
            for p in kv.as_array().into_iter().flatten() {
                let k = match &p[0] {
                    J::Number(n) => Key::Int(n.as_i64().unwrap_or(0) as Int),
                    J::String(s) => Key::Str(s.clone()),
                    _ => continue,
                };
                m.insert(k, p[1].as_i64().unwrap_or(0) as Int);
            }
        }
        for (s, kv) in d.get("texts").and_then(|t| t.as_object()).into_iter().flatten() {
            let m = w.texts.entry(s.clone()).or_default();
            for p in kv.as_array().into_iter().flatten() {
                let k = match &p[0] {
                    J::Number(n) => Key::Int(n.as_i64().unwrap_or(0) as Int),
                    J::String(s) => Key::Str(s.clone()),
                    _ => continue,
                };
                m.insert(k, p[1].as_str().unwrap_or("").to_string());
            }
        }
        w
    }

    /// The world for one call, read on demand inside the call's transaction.
    pub fn lazy(&self) -> World {
        World::lazy(Rc::new(SqliteStore { db: self.db.clone() }))
    }

    /// Write exactly what the call changed (accounts are never created by a call).
    fn write_dirty(&self, world: &World) -> Result<(), rusqlite::Error> {
        for (a, b) in &world.bal {
            self.db.execute("update account set balance=? where name=?", params![*b as i64, a])?;
        }
        // in declaration order, as Python's lazy world holds every declared map
        for x in self.defs.states.keys() {
            for (k, v) in world.maps.get(x).into_iter().flatten() {
                self.db.execute(
                    "insert into cell values(?,?,?) on conflict(state, key) do update set value=?",
                    params![x, key_json(k), *v as i64, *v as i64],
                )?;
            }
            for (k, v) in world.texts.get(x).into_iter().flatten() {
                self.db.execute(
                    "insert into textcell values(?,?,?) on conflict(state, key) do update set value=?",
                    params![x, key_json(k), v, v],
                )?;
            }
        }
        Ok(())
    }

    /// The whole world (replay, `GET /state`): never on the call path.
    pub fn load(&self) -> Result<World, rusqlite::Error> {
        let mut w = World::default();
        let mut st = self.db.prepare("select name, balance, owner from account")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))? {
            let (n, b, o) = row?;
            w.bal.insert(n.clone(), b as Int);
            w.own.insert(n, o);
        }
        let mut st = self.db.prepare("select state, key, value from cell")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
            let (s, k, v) = row?;
            w.maps.entry(s).or_default().insert(key_from_json(&k), v as Int);
        }
        let mut st = self.db.prepare("select state, key, value from textcell")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
            let (s, k, v) = row?;
            w.texts.entry(s).or_default().insert(key_from_json(&k), v);
        }
        Ok(w)
    }

    /// Write only what changed between two worlds (balances and map cells; maps never drop keys).
    fn write(&self, before: &World, after: &World) -> Result<(), rusqlite::Error> {
        for (a, b) in &after.bal {
            if before.bal.get(a) != Some(b) || !before.own.contains_key(a) {
                self.db.execute(
                    "insert into account values(?,?,?) on conflict(name) do update set balance=?",
                    params![a, *b as i64, after.own[a], *b as i64],
                )?;
            }
        }
        for (s, m) in &after.maps {
            let old = before.maps.get(s);
            for (k, v) in m {
                if old.and_then(|o| o.get(k)) != Some(v) {
                    self.db.execute(
                        "insert into cell values(?,?,?) on conflict(state, key) do update set value=?",
                        params![s, key_json(k), *v as i64, *v as i64],
                    )?;
                }
            }
        }
        for (s, m) in &after.texts {
            let old = before.texts.get(s);
            for (k, v) in m {
                if old.and_then(|o| o.get(k)) != Some(v) {
                    self.db.execute(
                        "insert into textcell values(?,?,?) on conflict(state, key) do update set value=?",
                        params![s, key_json(k), v, v],
                    )?;
                }
            }
        }
        Ok(())
    }

    // --- one top-level call: the only way anything changes

    /// Run `name args` atomically: {"status": ok|refused|error, ...} with the logged call's `call` seq.
    pub fn call(&mut self, name: &str, args: &[Value]) -> Result<CallResult, HostError> {
        let defs = self.defs;
        let Some(d) = defs.defs.get(name) else { return Err(HostError::NoDef(name.into())) };
        if self.gate && !self.reports.get(name).map_or(false, |r| r.clean) {
            return Err(HostError::Unproven(name.into()));
        }
        // the observation, read once per call; never before where a ticking call left the clock
        let now = (self.clock)().max(self.clock_floor);
        self.db.execute_batch("begin immediate").map_err(sql)?; // serialises calls, also across processes
        match self.call_in_tx(name, d.ret.is_some(), args, now) {
            Ok(res) => {
                self.db.execute_batch("commit").map_err(sql)?;
                Ok(res)
            }
            Err(e) => {
                let _ = self.db.execute_batch("rollback");
                Err(e)
            }
        }
    }

    fn call_in_tx(&mut self, name: &str, has_ret: bool, args: &[Value], now: Int) -> Result<CallResult, HostError> {
        let defs = self.defs;
        let mut world = self.lazy();
        world.clock = now;
        let providers = &mut self.providers;
        let provider: Provider = Box::new(move |n: &str, a: &[Value]| match providers.get_mut(n) {
            Some(p) => p(a),
            None => Err(ProvideError::Other("KeyError".into(), repr_str(n))),
        });
        let mut interp = Interp::with_provider(defs, Some(provider), 0);
        interp.budget = Some(ITERATION_LIMIT);
        let mut res = CallResult::new();
        match interp.run(name, args, &mut world) {
            // the budget only goes below 0 when it ran out
            Err(RunError::Kern(_)) if interp.budget.map_or(false, |b| b < 0) => {
                res.insert("status".into(), json!("error"));
                res.insert("fault".into(), json!("limit"));
                res.insert(
                    "error".into(),
                    json!(format!("iteration limit exceeded: more than {} loop and sum iterations in one call", ITERATION_LIMIT)),
                );
            }
            Err(RunError::Kern(e)) => {
                res.insert("status".into(), json!("error"));
                res.insert("error".into(), json!(e));
            }
            Err(RunError::Key(m)) => return Err(HostError::Fault("KeyError".into(), m)),
            Err(RunError::Type(m)) => return Err(HostError::Fault("TypeError".into(), m)),
            Err(RunError::Other(t, m)) => return Err(HostError::Fault(t, m)),
            Ok(Outcome::Ok) if out_of_range(&world, interp.last_result.as_ref().filter(|_| has_ret)).is_some() => {
                let what = out_of_range(&world, interp.last_result.as_ref().filter(|_| has_ret)).unwrap();
                res.insert("status".into(), json!("error"));
                res.insert("fault".into(), json!("range"));
                res.insert("error".into(), json!(format!("Int out of the host's range (i64): {}", what)));
            }
            Ok(Outcome::Fail(c, a)) if !a.iter().all(value_fits) => {
                res.insert("status".into(), json!("error"));
                res.insert("fault".into(), json!("range"));
                let shown: Vec<String> = a.iter().map(Value::repr).collect();
                res.insert("error".into(), json!(format!("Int out of the host's range (i64): refusal {} {}", c, shown.join(" "))));
            }
            Ok(Outcome::Ok) => {
                // tier ○, checked at runtime: at the keys this call wrote (sound: wf::invariant_problems)
                let mut skip = self.skip_proven && self.invariant_proven.get(name).copied().unwrap_or(false);
                if skip && self.audit_rate > 0.0 && self.audit_rng.random() < self.audit_rate {
                    skip = false;
                    self.audit_sampled += 1;
                }
                let broken = if skip {
                    vec![]
                } else {
                    let keys = world.written_keys(&defs.states);
                    interp
                        .broken_invariants(&mut world, Some(&keys))
                        .map_err(|e| HostError::Fault("KernError".into(), e.to_string()))?
                };
                if !broken.is_empty() && self.skip_proven && self.invariant_proven.get(name).copied().unwrap_or(false) {
                    self.audit_violations += 1; // a proven definition broke one: a host bug
                }
                if let Some((text, key)) = broken.first() {
                    res.insert("status".into(), json!("error"));
                    res.insert("fault".into(), json!("module"));
                    res.insert("error".into(), json!(format!("invariant broken: {} at {}", text, key.str())));
                } else {
                    res.insert("status".into(), json!("ok"));
                    res.insert("events".into(), J::Array(world.audit.iter().map(value_json).collect()));
                    if world.out.values().any(|e| !e.is_empty()) {
                        let out: Map<String, J> = world
                            .out
                            .iter()
                            .filter(|(_, e)| !e.is_empty())
                            .map(|(c, e)| (c.clone(), J::Array(e.iter().map(value_json).collect())))
                            .collect();
                        res.insert("out".into(), J::Object(out));
                    }
                    if has_ret {
                        res.insert("result".into(), interp.last_result.as_ref().map(value_json).unwrap_or(J::Null));
                    }
                }
            }
            Ok(Outcome::Fail(c, a)) => {
                res.insert("status".into(), json!("refused"));
                res.insert("refused".into(), json!(c));
                res.insert("args".into(), J::Array(a.iter().map(value_json).collect()));
            }
        }
        let logged: Map<String, J> =
            ["status", "refused", "args", "error"].iter().filter_map(|k| res.get(*k).map(|v| (k.to_string(), v.clone()))).collect();
        let observed = J::Array(
            interp
                .observations
                .iter()
                .map(|(n, a, v)| json!([n, J::Array(a.iter().map(value_json).collect()), value_json(v)]))
                .collect(),
        );
        drop(interp); // releases the providers before the database writes below
        let args_j = J::Array(args.iter().map(value_json).collect());
        self.db
            .execute(
                "insert into log(name, args, now, outcome, observed) values(?,?,?,?,?)",
                params![name, dumps(&args_j), now as i64, dumps(&J::Object(logged)), dumps(&observed)],
            )
            .map_err(sql)?;
        let seq = self.db.last_insert_rowid();
        if res["status"] == "ok" {
            self.answer(name, args, seq).map_err(sql)?;
            self.write_dirty(&world).map_err(sql)?;
            if world.clock > now + 1 {
                // the call used clock.tick (Interp::run steps the clock on by 1 after every call, so more than that
                // is ticks): the next call starts after its last instant, as in the interpreter
                let floor = world.clock;
                self.db
                    .execute("insert into meta values('clock_floor', ?) on conflict(k) do update set v=?", params![floor.to_string(), floor.to_string()])
                    .map_err(sql)?;
                self.clock_floor = floor;
            }
            for (ch, ev) in ordered_events(&world, defs) {
                self.db
                    .execute("insert into outbox(call, event, channel) values(?,?,?)", params![seq, dumps(&value_json(&ev)), ch])
                    .map_err(sql)?;
            }
        }
        res.insert("call".into(), json!(seq));
        Ok(res)
    }

    /// The open requests (`net` events no call has answered yet), oldest first: (outbox seq, event, the issuing
    /// call's `now`).
    fn open_requests(&self) -> Result<Vec<(i64, Value, Int)>, rusqlite::Error> {
        let mut st = self.db.prepare(
            "select o.seq, o.event, l.now from outbox o join log l on l.seq = o.call \
             where o.channel = 'net' and o.answered = 0 order by o.seq",
        )?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|(s, e, n)| (s, json_value(&serde_json::from_str(&e).unwrap_or(J::Null)), n as Int)).collect())
    }

    /// An ok call to a reply or fail definition answers the oldest open request it matches (in the call's
    /// transaction). Other definitions cost no statement.
    fn answer(&self, name: &str, args: &[Value], call: i64) -> Result<(), rusqlite::Error> {
        if !self.endpoints.values().any(|ep| ep.reply == name || ep.fail == name) {
            return Ok(());
        }
        for (seq, ev, _) in self.open_requests()? {
            let Value::Ctor(c, _) = &ev else { continue };
            if self.endpoints.get(c).map_or(false, |ep| ep.answers(name, args, &ev)) {
                self.db.execute("update outbox set answered=? where seq=?", params![call, seq])?;
                break;
            }
        }
        Ok(())
    }

    /// Requests past their deadline: the latest recorded `now` is at least the issuing call's `now` plus the
    /// endpoint's timeout. Each gives (outbox seq, the fail call).
    fn overdue(&self) -> Result<Vec<(i64, String, Vec<Value>)>, rusqlite::Error> {
        let open = self.open_requests()?;
        if open.is_empty() {
            return Ok(vec![]);
        }
        let latest: i64 = self.db.query_row("select coalesce(max(now), 0) from log", [], |r| r.get(0))?;
        let mut due = vec![];
        for (seq, ev, sent) in open {
            let Value::Ctor(c, _) = &ev else { continue };
            let Some(ep) = self.endpoints.get(c) else { continue };
            if latest as Int >= sent + ep.timeout {
                // load_config made sure the fail ARGS come from the event alone
                let args = ep.args_for(&ev, None, "fail").unwrap_or_default();
                due.push((seq, ep.fail.clone(), args));
            }
        }
        Ok(due)
    }

    /// Deliver committed outbox events to every sink, then mark them. At-least-once. Returns how many.
    pub fn flush(&mut self) -> Result<usize, HostError> {
        let rows: Vec<(i64, String, String)> = {
            let mut st = self.db.prepare("select seq, event, channel from outbox where delivered=0 order by seq").map_err(sql)?;
            let rows = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            rows
        };
        let mut n = 0;
        for (seq, ev, ch) in rows {
            let ev = json_value(&serde_json::from_str(&ev).unwrap_or(J::Null));
            if ch == "net" {
                // a request; whatever comes back is a later call
                if let Some(net) = self.net.as_mut() {
                    let replies = net.send(seq, &ev, &self.endpoints);
                    self.inbox.extend(replies);
                }
            }
            let sinks = if ch == "audit" { Some(&mut self.sinks) } else { self.channel_sinks.get_mut(&ch) };
            for sink in sinks.into_iter().flatten() {
                sink(seq, &ev);
            }
            self.db.execute("update outbox set delivered=1 where seq=?", [seq]).map_err(sql)?;
            n += 1;
        }
        Ok(n)
    }

    /// Run replies as new top-level calls (each one atomic and logged like any other call), then deliver what they
    /// emitted. Then every request past its deadline gets its `fail` call, once (S12); the result carries
    /// `"timeout": <the request's outbox seq>`. Returns the calls' results.
    pub fn pump(&mut self, extra: Vec<(String, Vec<Value>)>) -> Result<Vec<(String, Vec<Value>, CallResult)>, HostError> {
        self.inbox.extend(extra);
        let mut results = vec![];
        loop {
            while !self.inbox.is_empty() {
                let (name, args) = self.inbox.remove(0);
                let r = self.call(&name, &args)?;
                results.push((name, args, r));
                self.flush()?;
            }
            // after the replies that did arrive, so a reply in this same flush is never preempted
            let due = self.overdue().map_err(sql)?;
            if due.is_empty() {
                return Ok(results);
            }
            for (req, name, args) in due {
                let r = self.call(&name, &args);
                // once, whatever the outcome; marked after the call, so a crash in between fires it again
                // (at-least-once, like delivery). An ok call has already marked it with its own seq.
                let call = r.as_ref().ok().and_then(|r| r["call"].as_i64()).unwrap_or(-2);
                self.db.execute("update outbox set answered=? where seq=? and answered=0", params![call, req]).map_err(sql)?;
                let mut r = r?;
                r.insert("timeout".into(), json!(req));
                results.push((name, args, r));
                self.flush()?;
            }
        }
    }

    // --- determinism

    /// Re-run the call log from genesis with the recorded `now` values: the mismatches (empty = the database is
    /// exactly what the log implies).
    pub fn replay(&self) -> Result<Vec<String>, HostError> {
        let g: Option<String> = self.db.query_row("select v from meta where k='genesis'", [], |r| r.get(0)).optional().map_err(sql)?;
        let Some(g) = g else { return Ok(vec!["no genesis world".into()]) };
        let defs = self.defs;
        let mut w = Self::undump(&serde_json::from_str(&g).unwrap_or(J::Null));
        let (mut bad, mut events): (Vec<String>, Vec<(String, String)>) = (vec![], vec![]);
        let rows: Vec<(i64, String, String, i64, String, String)> = {
            let mut st = self.db.prepare("select seq, name, args, now, outcome, observed from log order by seq").map_err(sql)?;
            let rows = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            rows
        };
        // the recorded observations, in call order
        let queue: Rc<std::cell::RefCell<Vec<(String, Vec<Value>, Value)>>> = Rc::new(std::cell::RefCell::new(vec![]));
        let q = queue.clone();
        let provider: Provider = Box::new(move |n: &str, a: &[Value]| {
            let mut q = q.borrow_mut();
            if q.is_empty() || q[0].0 != n || q[0].1 != a {
                let args: Vec<String> = a.iter().map(Value::repr).collect();
                return Err(ProvideError::Other(
                    "RuntimeError".into(),
                    format!("replay: {} [{}] was not the next recorded observation", n, args.join(", ")),
                ));
            }
            Ok(q.remove(0).2)
        });
        let mut interp = Interp::with_provider(defs, Some(provider), 0);
        for (seq, name, args, now, logged, observed) in rows {
            let logged: J = serde_json::from_str(&logged).unwrap_or(J::Null);
            let obs: J = serde_json::from_str(&observed).unwrap_or(J::Array(vec![]));
            *queue.borrow_mut() = obs
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| {
                    (
                        o[0].as_str().unwrap_or("").to_string(),
                        o[1].as_array().into_iter().flatten().map(json_value).collect(),
                        json_value(&o[2]),
                    )
                })
                .collect();
            if logged["status"] == "error" {
                continue;
            }
            let snap = w.clone();
            w.clock = now as Int;
            w.audit.clear();
            w.out.clear();
            let args: Vec<Value> = match serde_json::from_str::<J>(&args) {
                Ok(J::Array(xs)) => xs.iter().map(json_value).collect(),
                _ => vec![],
            };
            let outcome = interp.run(&name, &args, &mut w).map_err(|e| HostError::Fault("KernError".into(), e.to_string()))?;
            let got = match &outcome {
                Outcome::Ok => "ok".to_string(),
                Outcome::Fail(c, _) => c.clone(),
            };
            let want = if logged["status"] == "ok" { "ok".to_string() } else { logged["refused"].as_str().unwrap_or("").to_string() };
            if got != want {
                bad.push(format!("call {} {}: logged {}, replay gives {}", seq, name, want, got));
                w = snap;
                continue;
            }
            events.extend(ordered_events(&w, defs).into_iter().map(|(ch, ev)| (ch, dumps(&value_json(&ev)))));
        }
        let now = self.load().map_err(sql)?;
        let replay_maps: IndexMap<String, IndexMap<Key, Int>> =
            w.maps.iter().filter(|(_, m)| !m.is_empty()).map(|(s, m)| (s.clone(), m.clone())).collect();
        let replay_texts: IndexMap<String, IndexMap<Key, String>> =
            w.texts.iter().filter(|(_, m)| !m.is_empty()).map(|(s, m)| (s.clone(), m.clone())).collect();
        if now.texts != replay_texts {
            let texts = |m: &IndexMap<String, IndexMap<Key, String>>| {
                dict_repr(m.iter(), |x| repr_str(x), |kv| dict_repr(kv.iter(), |k| key_repr(k), |v| repr_str(v)))
            };
            bad.push(format!("text state differs: db {}, replay {}", texts(&now.texts), texts(&replay_texts)));
        }
        if now.bal != w.bal || now.maps != replay_maps {
            let bal = |w: &World| dict_repr(w.bal.iter(), |a| repr_str(a), |b| b.to_string());
            let maps = |m: &IndexMap<String, IndexMap<Key, Int>>| {
                dict_repr(m.iter(), |x| repr_str(x), |kv| dict_repr(kv.iter(), |k| key_repr(k), |v| v.to_string()))
            };
            bad.push(format!("state differs: db {} {}, replay {} {}", bal(&now), maps(&now.maps), bal(&w), maps(&w.maps)));
        }
        let stored: Vec<(String, String)> = {
            let mut st = self.db.prepare("select channel, event from outbox order by seq").map_err(sql)?;
            let rows = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            rows
        };
        if stored != events {
            bad.push(format!("outbox differs: {} stored, {} from replay", stored.len(), events.len()));
        }
        Ok(bad)
    }

    pub fn log_count(&self) -> i64 {
        self.db.query_row("select count(*) from log", [], |r| r.get(0)).unwrap_or(0)
    }

    // --- the permission manifest: inferred, never declared

    pub fn manifest(&self) -> Map<String, J> {
        let mut out = Map::new();
        for (n, r) in &self.reports {
            let d = &self.defs.defs[n];
            let gs = &r.guarantees;
            let mut fs: Vec<String> = fails(self.defs, d).into_keys().collect();
            fs.sort();
            out.insert(
                n.clone(),
                json!({
                    "params": d.params.iter().map(|(p, t)| format!("{}:{}", p, t)).collect::<Vec<_>>(),
                    "uses": uses(self.defs, d).into_iter().collect::<Vec<_>>(),
                    "fails": fs,
                    "served": r.clean || !self.gate,
                    "proven": format!("{}/{}", gs.iter().filter(|g| g.status == GStatus::Proven).count(), gs.len()),
                    "runtime_invariant_check": if self.runtime_check_skipped(n) { "removed (proven)" } else { "on" },
                }),
            );
        }
        out
    }
}

pub fn host_effect(root: &str) -> &'static str {
    match root {
        "ledger" => "SQLite table `account`, one transaction per call",
        "clock" => {
            "wall clock, seconds; read once per call, recorded in `log`; clock.tick adds exactly 1 within the call, and \
             the next call's time is never earlier than where a ticking call ended"
        }
        "audit" => "SQLite table `outbox`; delivered to stdout after commit (at-least-once)",
        "rand" => {
            "assumed handler rand.below: the OS random source (secrets); every result checked against its assumed spec \
             and recorded in `log.observed` for replay"
        }
        "console" => "SQLite table `outbox` (channel console); one line per event on stdout after commit (at-least-once)",
        "net" => {
            "SQLite table `outbox` (channel net); each event is an HTTP request from the host config, sent after commit \
             (at-least-once); the reply or failure arrives as a new top-level call, and a request with no answer by its \
             deadline gets its failure call"
        }
        _ => "SQLite table `cell`, one transaction per call",
    }
}

pub fn manifest_text(host: &Host) -> String {
    let m = host.manifest();
    let allu: BTreeSet<String> =
        m.values().flat_map(|v| v["uses"].as_array().unwrap().iter().map(|u| u.as_str().unwrap().to_string())).collect();
    let mut lines = vec![format!("module may use: {}", allu.iter().cloned().collect::<Vec<_>>().join(", ")), "host provides:".into()];
    let roots: BTreeSet<String> = allu.iter().map(|u| u.split('.').next().unwrap().to_string()).collect();
    for root in roots {
        lines.push(format!("  {:<8} {}", root, host_effect(&root)));
    }
    let mut eps: Vec<(&String, &Endpoint)> = host.endpoints.iter().collect();
    eps.sort_by(|a, b| a.0.cmp(b.0));
    for (ctor, ep) in eps {
        let mut pattern = ep.url.clone();
        for i in 1..10 {
            pattern = pattern.replace(&format!("{{{}}}", i), "*");
        }
        lines.push(format!(
            "  net: {} -> {} {}  reply {}, fail {}; no answer within {}s -> {}",
            ctor, ep.method, pattern, ep.reply, ep.fail, ep.timeout, ep.fail
        ));
    }
    lines.push("nothing else: the module cannot name any other effect or endpoint.".into());
    let per_call = if host.skip_proven {
        format!(
            "per call, skipped where every invariant goal is proven (--skip-proven); sampled audit rate {} ({} sampled, {} violations)",
            py_float(host.audit_rate),
            host.audit_sampled,
            host.audit_violations
        )
    } else {
        "per call, on for every definition (the default; --skip-proven removes proven ones)".to_string()
    };
    lines.push(format!("runtime checks: invariants audited in full at startup (passed); {}", per_call));
    lines.push(format!("iteration limit: {} loop and sum iterations per call; past it the call is a fault, undone", ITERATION_LIMIT));
    lines.push(format!(
        "call depth: definitions nest at most {} deep per call (recursion included); deeper is an error, undone",
        crate::run::MAX_CALL_DEPTH
    ));
    lines.push(String::new());
    for (n, v) in &m {
        let mark = if v["served"].as_bool().unwrap() { "serve" } else { "BLOCK" };
        let join = |k: &str| {
            let xs: Vec<&str> = v[k].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
            if xs.is_empty() { "-".to_string() } else { xs.join(", ") }
        };
        lines.push(format!(
            "{} {} {}  proven {}  uses {}  fails {}  runtime invariant check {}",
            mark,
            n,
            v["params"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect::<Vec<_>>().join(" "),
            v["proven"].as_str().unwrap(),
            join("uses"),
            join("fails"),
            v["runtime_invariant_check"].as_str().unwrap()
        ));
    }
    lines.join("\n")
}

/// Python's str() of a float (the audit rate).
fn py_float(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e16 {
        format!("{:.1}", x)
    } else {
        format!("{}", x)
    }
}

pub const ASSUMPTIONS: [&str; 5] = [
    "Calls run one at a time (a lock in this process; `begin immediate` across processes). The proofs assume serial \
     top-level calls.",
    "Every event is delivered at least once, after its call commits. A crash between delivering and marking an event \
     sends it again (docs/HOST.md has the measured count). Consumers dedupe on `seq`.",
    "The order of events across channels is audit first, then channels in declaration order. Specs fix the order \
     within a channel only.",
    "`clock.now` is the wall clock in seconds, read once per call and recorded for replay.",
    "An Int is an i64 here (the proofs use unbounded integers): a call that would store, emit or return an Int \
     outside i64 is undone as a host fault, and input outside i64 is refused.",
];
pub const NET_ASSUMPTIONS: [&str; 2] = [
    "A network reply becomes a new top-level call. The host decodes each `$.field` as an Int; a timeout, a non-2xx \
     status or anything that isn't an Int becomes the `fail` call. Kern never sees bytes.",
    "A request that no call to its reply or fail definition has answered (returned ok) within its deadline gets its \
     `fail` call, once, as a new top-level call: a reply that never arrives, or one the module refuses. The deadline \
     is in seconds of the recorded clock (below), checked after each call's replies, so an idle host fires it at its \
     next call. A reply that arrives later is an ordinary call the module may refuse.",
];

/// 10000000 -> "10,000,000" (the page's wording)
fn format_num(n: Int) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The data for render::host_html: effects the module uses, endpoints, assumptions.
pub fn host_facts(host: &Host) -> HostInfo {
    let used: BTreeSet<String> = host
        .manifest()
        .values()
        .flat_map(|v| v["uses"].as_array().unwrap().iter().map(|u| u.as_str().unwrap().split('.').next().unwrap().to_string()))
        .collect();
    let mut eps: Vec<(&String, &Endpoint)> = host.endpoints.iter().collect();
    eps.sort_by(|a, b| a.0.cmp(b.0));
    let mut assumptions: Vec<String> = ASSUMPTIONS.iter().map(|s| s.to_string()).collect();
    assumptions.push(format!(
        "A call may run at most {} loop and sum iterations (`hi - lo` is counted when a loop or sum starts); past \
         that it is undone as a host fault. The proofs say a call succeeds, not how long it takes.",
        format_num(ITERATION_LIMIT)
    ));
    assumptions.push(format!(
        "Definitions nest at most {} deep in one call, the top-level one included (recursion is licensed by its proven \
         measure; this bounds the stack); deeper, the call is undone as an error.",
        crate::run::MAX_CALL_DEPTH
    ));
    if !host.endpoints.is_empty() {
        assumptions.extend(NET_ASSUMPTIONS.iter().map(|s| s.to_string()));
        // the effective values, not the default: the page says what this host will do
        let deadlines: Vec<String> = eps
            .iter()
            .map(|(c, ep)| {
                let how = if ep.src.split(';').count() == 4 { "" } else { " (the default)" };
                format!("{} {} s{}", c, ep.timeout, how)
            })
            .collect();
        assumptions.push(format!("Request deadlines: {}.", deadlines.join(", ")));
    }
    HostInfo {
        effects: used.iter().map(|u| (u.clone(), host_effect(u).to_string())).collect(),
        endpoints: eps
            .iter()
            .map(|(c, ep)| ((*c).clone(), ep.method.clone(), ep.url.clone(), ep.reply.clone(), ep.fail.clone()))
            .collect(),
        assumptions,
    }
}

fn state_json(w: &World) -> J {
    json!({
        "balances": w.bal.iter().map(|(a, b)| (a.clone(), json!(*b as i64))).collect::<Map<String, J>>(),
        "state": w
            .maps
            .iter()
            .map(|(s, m)| (s.clone(), J::Object(m.iter().map(|(k, v)| (key_str(k), json!(*v as i64))).collect())))
            .chain(w.texts.iter().map(|(s, m)| (s.clone(), J::Object(m.iter().map(|(k, v)| (key_str(k), json!(v))).collect()))))
            .collect::<Map<String, J>>(),
    })
}

fn parse_args(text: &[&str]) -> Vec<Value> {
    text.iter()
        .map(|x| {
            let digits = x.trim_start_matches('-');
            // the host's Int is an i64 (S20): a longer number stays text, which no Int parameter accepts
            match x.parse::<i64>() {
                Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => Value::Int(n as Int),
                _ => Value::Str(x.to_string()),
            }
        })
        .collect()
}

fn said(r: &CallResult) -> String {
    match r["status"].as_str() {
        // a definition that returns a value shows it, as an `eg` line asserts it (`-> ok N`)
        Some("ok") => match r.get("result") {
            Some(v) if !v.is_null() => format!("ok {}", json_value(v).str()),
            _ => "ok".into(),
        },
        Some("refused") => {
            let mut parts = vec!["refused".to_string(), r["refused"].as_str().unwrap_or("").to_string()];
            parts.extend(r["args"].as_array().into_iter().flatten().map(|a| json_value(a).str()));
            parts.join(" ")
        }
        _ => format!("error: {}", r["error"].as_str().unwrap_or("")),
    }
}

/// A Text as the console reads it (`"…"`, `\\` and `\"` escaped), with line breaks escaped too (`\n`, `\r`): a
/// reply's Text may come from anywhere, and a reply line must stay one line.
fn line_quote(t: &str) -> String {
    crate::syntax::quote(t).replace('\n', "\\n").replace('\r', "\\r")
}

/// Run network replies as new calls (after the request's call has committed and its events went out), and report
/// each one. A reply to an unproven definition is skipped, as any call is.
pub fn run_replies(host: &mut Host, out: &mut dyn FnMut(String)) -> Result<(), HostError> {
    match host.pump(vec![]) {
        Err(HostError::Unproven(n)) => {
            out(format!("error: reply {} is not proven clean; this host does not run it", n));
            Ok(())
        }
        Err(e) => Err(e),
        Ok(results) => {
            for (name, args, r) in results {
                // a Text argument quoted as the console reads it, newlines escaped, so one reply is one line
                let tys: Vec<String> = host.defs.defs.get(&name).map(|d| d.params.iter().map(|(_, t)| t.clone()).collect()).unwrap_or_default();
                let mut call = vec![name];
                call.extend(args.iter().enumerate().map(|(j, a)| match (a, tys.get(j).map(String::as_str)) {
                    (Value::Str(t), Some("Text")) => line_quote(t),
                    _ => a.str(),
                }));
                let how = if r.contains_key("timeout") { "timeout" } else { "reply" };
                out(format!("{} {} -> {}", how, call.join(" "), said(&r)));
            }
            Ok(())
        }
    }
}

/// The console: one call per line, the outcome (and any reply calls) after each.
pub fn repl(host: &mut Host, lines: impl Iterator<Item = String>, out: &mut dyn FnMut(String)) -> Result<(), HostError> {
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "state" {
            let w = host.load().map_err(sql)?;
            out(dumps(&state_json(&w)));
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let (name, args) = if line.contains('"') {
            // a quoted Text argument: the eg grammar (`post 3 "hello world"`)
            match crate::syntax::parse_call("", line) {
                Ok(ex) => (ex.call.0.clone(), ex.call.1.iter().map(Value::from_key).collect()),
                Err(e) => {
                    out(format!("error: {}", e.0));
                    continue;
                }
            }
        } else {
            (parts[0].to_string(), parse_args(&parts[1..]))
        };
        let name = name.as_str();
        match host.call(name, &args) {
            Err(HostError::NoDef(_)) => {
                out(format!("error: no definition named {}", name));
                continue;
            }
            Err(HostError::Unproven(_)) => {
                out(format!("error: {} is not proven clean; this host does not run it", name));
                continue;
            }
            Err(e) => return Err(e),
            Ok(r) => {
                out(match r["status"].as_str() {
                    Some("ok") => match r.get("result") {
                        Some(v) => format!("ok {}", json_value(v).str()),
                        None => "ok".into(),
                    },
                    _ => said(&r),
                });
            }
        }
        host.flush()?;
        run_replies(host, out)?;
    }
    Ok(())
}

// --- the network: POST /NAME with a JSON object of named parameters

fn respond(req: tiny_http::Request, code: u16, body: &J) {
    let data = dumps(body);
    let header = tiny_http::Header::from_bytes(&b"content-type"[..], &b"application/json"[..]).unwrap();
    let _ = req.respond(tiny_http::Response::from_string(data).with_status_code(code).with_header(header));
}

/// The call's arguments from a JSON body with exactly the definition's parameters. A record parameter (flattened at
/// load time to `p.x`, `p.y`: syntax::flatten_records) is one object with exactly its fields: `{"p": {"x": 1, "y": 2}}`;
/// a nested record (`l.a.x`) is an object inside it, `{"l": {"a": {"x": 1, ...}, ...}}`.
pub fn http_args(params: &[(String, String)], body: &J) -> Result<Vec<Value>, String> {
    // the parameters as a tree of field paths, in declaration order: an inner node is a record, a leaf a value
    #[derive(Default)]
    struct Shape(IndexMap<String, Shape>);
    let mut root = Shape::default();
    for (p, _) in params {
        let mut node = &mut root;
        for part in p.split('.') {
            node = node.0.entry(part.to_string()).or_default();
        }
    }
    // every record node, with its dotted path, outermost first (for the message)
    fn records<'a>(path: &str, s: &'a Shape, out: &mut Vec<(String, &'a Shape)>) {
        for (n, c) in &s.0 {
            if !c.0.is_empty() {
                let p = if path.is_empty() { n.clone() } else { format!("{}.{}", path, n) };
                out.push((p.clone(), c));
                records(&p, c, out);
            }
        }
    }
    fn fits(s: &Shape, v: &J) -> bool {
        let Some(o) = v.as_object() else { return false };
        o.len() == s.0.len() && s.0.iter().all(|(n, c)| o.get(n).map_or(false, |x| c.0.is_empty() || fits(c, x)))
    }
    if !fits(&root, body) {
        let list = |s: &Shape| format!("[{}]", s.0.keys().map(|w| repr_str(w)).collect::<Vec<_>>().join(", "));
        let mut msg = format!("want a JSON object with exactly {}", list(&root));
        let mut recs = vec![];
        records("", &root, &mut recs);
        for (p, c) in recs {
            msg.push_str(&format!("; {} an object with exactly {}", repr_str(&p), list(c)));
        }
        return Err(msg);
    }
    Ok(params.iter().map(|(p, _)| json_value(p.split('.').fold(body, |v, part| &v[part]))).collect())
}

/// One HTTP request; Err only for a host fault outside a call.
pub fn handle_http(host: &mut Host, mut req: tiny_http::Request) -> Result<(), HostError> {
    let path = req.url().to_string();
    if *req.method() == tiny_http::Method::Get {
        match path.as_str() {
            "/manifest" => respond(req, 200, &J::Object(host.manifest())),
            "/state" => {
                let w = host.load().map_err(sql)?;
                respond(req, 200, &state_json(&w));
            }
            _ => respond(req, 404, &json!({"error": "GET /manifest or /state; POST /NAME"})),
        }
        return Ok(());
    }
    if *req.method() != tiny_http::Method::Post {
        respond(req, 501, &json!({"error": "GET /manifest or /state; POST /NAME"}));
        return Ok(());
    }
    let name = path.trim_matches('/').to_string();
    let defs = host.defs;
    let Some(d) = defs.defs.get(&name) else {
        respond(req, 404, &json!({"error": format!("no definition named {}", repr_str(&name))}));
        return Ok(());
    };
    let mut raw = String::new();
    let _ = req.as_reader().read_to_string(&mut raw);
    let body: J = if raw.is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&raw) {
            Ok(b) => b,
            Err(_) => {
                respond(req, 400, &json!({"error": "body is not JSON"}));
                return Ok(());
            }
        }
    };
    let args = match http_args(&d.params, &body) {
        Ok(a) => a, // types are checked by Interp::run
        Err(e) => {
            respond(req, 400, &json!({"error": e}));
            return Ok(());
        }
    };
    match host.call(&name, &args) {
        Err(HostError::Unproven(_)) => {
            respond(req, 403, &json!({"error": format!("{} is not proven clean; this host does not run it", name)}));
        }
        Err(e) => respond(req, 500, &json!({"error": format!("host: {}", e)})), // a host fault; the call rolled back
        Ok(r) => {
            let code = if matches!(r.get("fault").and_then(|f| f.as_str()), Some("module") | Some("range") | Some("limit")) {
                500 // ○ tier fired, or an Int left the host's range (S20): not the caller's input
            } else {
                match r["status"].as_str() {
                    Some("ok") => 200,
                    Some("refused") => 409,
                    _ => 400,
                }
            };
            respond(req, code, &J::Object(r));
            host.flush()?; // after commit, after the reply
            run_replies(host, &mut |_| {})?; // network replies arrive as new calls
        }
    }
    Ok(())
}

/// Serve HTTP, one request at a time (calls are serial, as the proofs assume). `ready` gets the bound port.
pub fn serve(host: &mut Host, port: u16, bind: &str, ready: Option<&mut dyn FnMut(u16)>) -> Result<(), String> {
    let srv = tiny_http::Server::http((bind, port)).map_err(|e| e.to_string())?;
    let bound = srv.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
    match ready {
        Some(f) => f(bound),
        None => println!("serving on http://{}:{}", bind, bound),
    }
    let _ = std::io::stdout().flush();
    for req in srv.incoming_requests() {
        handle_http(host, req).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- the command line

pub const COMMANDS: [&str; 5] = ["serve", "repl", "replay", "manifest", "page"];
/// option -> takes a value
pub const OPTIONS: [(&str, bool); 10] = [
    ("--html", true),
    ("--db", true),
    ("--world", true),
    ("--port", true),
    ("--bind", true),
    ("--no-gate", false),
    ("--config", true),
    ("--skip-proven", false),
    ("--audit-rate", true),
    ("--why", true),
];

const USAGE: &str = "usage: kern host [-h] [--html HTML] [--db DB] [--world WORLD] [--port PORT] [--bind BIND] [--no-gate]
                 [--config CONFIG] [--skip-proven] [--audit-rate AUDIT_RATE] [--why WHYFILE]
                 {serve,repl,replay,manifest,page} file";

/// `analyse` for the host: Python's analyse(defs, mutation=False) (and mutation=True for `page`).
pub type Analyse<'a> = &'a dyn Fn(&Module, bool) -> Result<IndexMap<String, Report>, String>;

/// `kern host ...`: returns the exit code.
pub fn main(argv: &[String], analyse: Analyse) -> i32 {
    let mut opts: IndexMap<&str, String> = IndexMap::new();
    let (mut pos, mut i) = (vec![], 0);
    while i < argv.len() {
        let a = &argv[i];
        i += 1;
        if a == "-h" || a == "--help" {
            println!("{}", USAGE);
            return 0;
        }
        if a.starts_with("--") {
            let (flag, inline) = match a.split_once('=') {
                Some((f, v)) => (f, Some(v.to_string())),
                None => (a.as_str(), None),
            };
            let Some((name, takes)) = OPTIONS.iter().find(|(o, _)| *o == flag) else {
                eprintln!("{}\nkern host: error: unrecognized arguments: {}", USAGE, a);
                return 2;
            };
            if *takes {
                let v = match inline {
                    Some(v) => v,
                    None if i < argv.len() => {
                        i += 1;
                        argv[i - 1].clone()
                    }
                    None => {
                        eprintln!("{}\nkern host: error: argument {}: expected one argument", USAGE, flag);
                        return 2;
                    }
                };
                opts.insert(name, v);
            } else {
                opts.insert(name, String::new());
            }
        } else {
            pos.push(a.clone());
        }
    }
    if pos.len() != 2 || !COMMANDS.contains(&pos[0].as_str()) {
        eprintln!("{}\nkern host: error: want a command ({}) and a file", USAGE, COMMANDS.join(", "));
        return 2;
    }
    let (cmd, file) = (pos[0].as_str(), pos[1].as_str());
    let port: u16 = match opts.get("--port").map(|p| p.parse()) {
        None => 8080,
        Some(Ok(p)) => p,
        Some(Err(_)) => {
            eprintln!("{}\nkern host: error: argument --port: invalid int value", USAGE);
            return 2;
        }
    };
    let audit_rate: f64 = opts.get("--audit-rate").and_then(|r| r.parse().ok()).unwrap_or(0.0);
    let defs = match crate::package::load(file, &IndexMap::new()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}: {}", file, e);
            return 2;
        }
    };
    let cfg = opts.get("--config").cloned().unwrap_or_else(|| crate::package::store_path(file, ".host"));
    let endpoints = if std::path::Path::new(&cfg).exists() {
        match crate::package::read_text(&cfg).map_err(|e| ConfigError(e.to_string())).and_then(|t| load_config(&t, &defs)) {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("{}: host refuses to start: {}", file, e.0);
                return 2;
            }
        }
    } else {
        None
    };
    let reports = match analyse(&defs, false) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}: {}", file, e);
            return 2;
        }
    };
    let options = Options {
        db: opts.get("--db").cloned().unwrap_or_else(|| ":memory:".into()),
        gate: !opts.contains_key("--no-gate"),
        endpoints,
        skip_proven: opts.contains_key("--skip-proven"),
        audit_rate,
        ..Default::default()
    };
    let mut host = match Host::new(&defs, reports, options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{}: host refuses to start: {}", file, e.0);
            return 2;
        }
    };
    let world = opts.get("--world").cloned();
    let init = match &world {
        Some(w) => host.init(w).map(|fresh| {
            if !fresh {
                eprintln!("note: database already has a world; --world ignored");
            }
        }),
        // a new database: an empty genesis world, so `replay` has a start
        None if cmd == "serve" || cmd == "repl" => host.init("").map(|_| ()),
        None => Ok(()),
    };
    if let Err(e) = init {
        eprintln!("{}: {}", file, e);
        return 2;
    }
    let fault = |e: HostError| {
        eprintln!("{}: host fault: {}", file, e);
        1
    };
    match cmd {
        "manifest" => {
            println!("{}", manifest_text(&host));
            0
        }
        "page" => {
            // the module's rationale, from the same default file `kern check` reads (`<module>.why`, `<root>/kern.why`)
            let whyfile = opts.get("--why").cloned().unwrap_or_else(|| crate::package::store_path(file, ".why"));
            let store = match crate::why::load(&whyfile) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {}", e);
                    return 2;
                }
            };
            let whys: IndexMap<String, crate::why::Status> =
                defs.defs.iter().map(|(n, d)| (n.clone(), crate::why::status(&store, d, &defs))).collect();
            let orphans = crate::why::orphans(&store, &defs);
            let reports = match analyse(&defs, true) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}: {}", file, e);
                    return 2;
                }
            };
            let out = opts.get("--html").cloned().unwrap_or_else(|| "proof.html".into());
            let page = crate::render::page(&reports, &defs, &whys, file, &orphans, Some(&host_facts(&host)));
            if let Some(dir) = std::path::Path::new(&out).parent().filter(|d| !d.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&out, page) {
                eprintln!("error: {}: {}", out, e);
                return 2;
            }
            println!("proof page with host section: {}", out);
            0
        }
        "replay" => match host.replay() {
            Err(e) => fault(e),
            Ok(bad) => {
                if bad.is_empty() {
                    println!("replay of {} logged calls matches the database", host.log_count());
                    0
                } else {
                    println!("{}", bad.join("\n"));
                    1
                }
            }
        },
        "repl" => {
            // anything left undelivered by a crash goes out first
            let mut print = |s: String| println!("{}", s);
            if let Err(e) = host.flush().and_then(|_| run_replies(&mut host, &mut print)) {
                return fault(e);
            }
            let stdin = std::io::stdin();
            let lines = stdin.lock().lines().map_while(Result::ok);
            match repl(&mut host, lines, &mut print) {
                Ok(()) => 0,
                Err(e) => fault(e),
            }
        }
        _ => {
            if let Err(e) = host.flush().and_then(|_| run_replies(&mut host, &mut |_| {})) {
                return fault(e);
            }
            let bind = opts.get("--bind").cloned().unwrap_or_else(|| "127.0.0.1".into());
            match serve(&mut host, port, &bind, None) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{}: {}", file, e);
                    1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! S12: a request no call answers gets its `fail` call at its deadline. kern/tests/host.rs (tester's) and
    //! kern/tests/shortcomings.rs (the gap test) cover the rest of the host.
    use super::*;
    use serde_json::json;

    const LIMITS: &str = include_str!("../../examples/limits/limits.k");
    const CONFIG: &str = "Fetch -> GET http://127.0.0.1:9/limit/{1}; reply got_limit {1} {2} $.value; fail limit_failed {1} {2}";

    fn module() -> Module {
        let m = crate::syntax::parse_module(LIMITS, true).unwrap();
        crate::wf::check_module(&m).unwrap();
        m
    }

    /// Replies the test hands over: `send` answers with whatever is queued, else nothing.
    struct Scripted(Rc<std::cell::RefCell<Vec<(String, Vec<Value>)>>>);
    impl Net for Scripted {
        fn send(&mut self, _: i64, _: &Value, _: &IndexMap<String, Endpoint>) -> Vec<(String, Vec<Value>)> {
            std::mem::take(&mut *self.0.borrow_mut())
        }
    }

    /// A host whose clock moves `step` per call, ungated (the proofs are not what this tests).
    fn host<'m>(defs: &'m Module, db: &str, config: &str, step: Int, net: Box<dyn Net>) -> Host<'m> {
        let mut t = 0;
        let o = Options {
            db: db.into(),
            clock: Some(Box::new(move || {
                t += step;
                t
            })),
            sinks: Some(vec![]),
            gate: false,
            endpoints: Some(load_config(config, defs).unwrap()),
            net: Some(net),
            ..Default::default()
        };
        let mut h = Host::new(defs, IndexMap::new(), o).unwrap();
        h.init("A:500@u1 B:500@u2 K:0@u9").unwrap();
        h
    }

    fn a() -> Value {
        Value::Str("A".into())
    }

    fn pending(h: &Host) -> Option<Int> {
        h.load().unwrap().maps.get("pending").and_then(|m| m.get(&Key::Str("A".into())).copied())
    }

    /// One tick: an unrelated call, flush, pump. The names of the calls pump ran, with "timeout " in front of timeouts.
    fn tick(h: &mut Host) -> Vec<String> {
        h.call("reset_day", &[Value::Str("B".into())]).unwrap();
        h.flush().unwrap();
        h.pump(vec![]).unwrap().into_iter().map(|(n, _, r)| if r.contains_key("timeout") { format!("timeout {n}") } else { n }).collect()
    }

    #[test]
    fn deadline_is_stated_or_defaulted() {
        let defs = module();
        let eps = load_config(CONFIG, &defs).unwrap();
        assert_eq!(eps["Fetch"].timeout, DEFAULT_TIMEOUT);
        assert_eq!(load_config(&format!("{CONFIG}; timeout 30"), &defs).unwrap()["Fetch"].timeout, 30);
        for bad in ["; timeout 0", "; timeout -5", "; timeout x", "; after 30", "; timeout 30; timeout 40"] {
            assert!(load_config(&format!("{CONFIG}{bad}"), &defs).is_err(), "{bad}");
        }
        // the fail call must be buildable from the event alone, since it also runs when nothing came back
        let e = load_config(&CONFIG.replace("fail limit_failed {1} {2}", "fail got_limit {1} {2} $.value"), &defs).unwrap_err().0;
        assert!(e.contains("a failure has no reply body"), "{e}");
        let e = load_config(&CONFIG.replace("fail limit_failed {1} {2}", "fail limit_failed {1} {3}"), &defs).unwrap_err().0;
        assert!(e.contains("Fetch has 2 arguments"), "{e}");
    }

    #[test]
    fn silence_times_out_once_at_the_deadline() {
        let defs = module();
        let mut h = host(&defs, ":memory:", &format!("{CONFIG}; timeout 100"), 30, Box::new(Scripted(Default::default())));
        assert_eq!(h.call("request_limit", &[a()]).unwrap()["status"], "ok"); // now 30, deadline 130
        let ran: Vec<Vec<String>> = (0..5).map(|_| tick(&mut h)).collect(); // now 60, 90, 120, 150 (+ the timeout), ...
        assert_eq!(ran, [vec![], vec![], vec![], vec!["timeout limit_failed".to_string()], vec![]]);
        assert_eq!(pending(&h), Some(0));
        assert_eq!(h.call("request_limit", &[a()]).unwrap()["status"], "ok", "progress: a new request is allowed");
        assert!(h.replay().unwrap().is_empty());
        assert!(manifest_text(&h).contains("reply got_limit, fail limit_failed; no answer within 100s -> limit_failed"));
        assert!(host_facts(&h).assumptions.contains(&"Request deadlines: Fetch 100 s.".to_string()));
    }

    #[test]
    fn an_ok_reply_answers_the_request() {
        let defs = module();
        let q: Rc<std::cell::RefCell<Vec<(String, Vec<Value>)>>> = Default::default();
        let mut h = host(&defs, ":memory:", CONFIG, 100, Box::new(Scripted(q.clone())));
        h.call("request_limit", &[a()]).unwrap();
        q.borrow_mut().push(("got_limit".into(), vec![a(), Value::Int(1), Value::Int(250)]));
        assert_eq!(tick(&mut h), ["got_limit"]);
        assert!((0..10).all(|_| tick(&mut h).is_empty()), "answered: no timeout, ever");
        assert_eq!(h.load().unwrap().maps["limit"][&Key::Str("A".into())], 250);
        assert!(host_facts(&h).assumptions.contains(&"Request deadlines: Fetch 300 s (the default).".to_string()));
    }

    #[test]
    fn a_refused_reply_does_not_answer_and_a_late_one_is_stale() {
        let defs = module();
        let q: Rc<std::cell::RefCell<Vec<(String, Vec<Value>)>>> = Default::default();
        let mut h = host(&defs, ":memory:", CONFIG, 100, Box::new(Scripted(q.clone())));
        h.call("request_limit", &[a()]).unwrap();
        q.borrow_mut().push(("got_limit".into(), vec![a(), Value::Int(1), Value::Int(-7)])); // BadValue
        assert_eq!(tick(&mut h), ["got_limit"]);
        let mut ran = vec![];
        for _ in 0..4 {
            ran.extend(tick(&mut h));
        }
        assert_eq!(ran, ["timeout limit_failed"]);
        assert_eq!(pending(&h), Some(0));
        // the reply that finally shows up is an ordinary call, refused as stale
        let r = h.pump(vec![("got_limit".into(), vec![a(), Value::Int(1), Value::Int(250)])]).unwrap();
        assert_eq!(r[0].2["refused"], "Stale");
        assert!(h.replay().unwrap().is_empty());
    }

    #[test]
    fn the_deadline_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("kern-s12-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("h.db").to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&db);
        let defs = module();
        {
            let mut h = host(&defs, &db, CONFIG, 100, Box::new(Scripted(Default::default())));
            h.call("request_limit", &[a()]).unwrap();
            h.flush().unwrap(); // sent; the process dies before any answer
        }
        let mut h = host(&defs, &db, CONFIG, 1000, Box::new(Scripted(Default::default())));
        assert_eq!(tick(&mut h), ["timeout limit_failed"]);
        assert_eq!(pending(&h), Some(0));
        assert!(h.replay().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// HOST.md's liveness measurement (item 5's hostile network, with deadlines): `cargo test --release -p kern --lib
    /// host::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_liveness_on_a_hostile_network() {
        let defs = module();
        let (mut stuck, mut stuck_without, mut timeouts, mut faults, mut mismatches, mut calls) = (0, 0, 0, 0, 0, 0);
        for run in 0..1000 {
            let rng = Rc::new(std::cell::RefCell::new(Random::from_int(run)));
            let r2 = rng.clone();
            let values = Box::new(move |_: &Value| {
                let mut r = r2.borrow_mut();
                let v = r.randint(0, 900);
                r.choice(&[json!({"value": v}), json!({"value": -1}), json!({"value": "x"}), json!({})]).clone()
            });
            let mut h = host(&defs, ":memory:", &format!("{CONFIG}; timeout 20"), 1, Box::new(SimNet::new(values, run)));
            for _ in 0..25 {
                let who = Value::Str(rng.borrow_mut().choice(&["A", "B", "K"]).to_string());
                let op = rng.borrow_mut().random();
                let r = if op < 0.35 {
                    h.call("request_limit", &[who]).unwrap()
                } else if op < 0.65 {
                    let amount = *rng.borrow_mut().choice(&[1, 50, 200, 600]);
                    h.call("withdraw", &[who, Value::Str("K".into()), Value::Int(amount)]).unwrap()
                } else {
                    h.call("reset_day", &[who]).unwrap()
                };
                calls += 1;
                faults += (r.get("fault") == Some(&json!("module"))) as u64;
                h.flush().unwrap();
                let held = if rng.borrow_mut().random() < 0.5 { h.net.as_mut().unwrap().release() } else { vec![] };
                for (_, _, r) in h.pump(held).unwrap() {
                    calls += 1;
                    timeouts += r.contains_key("timeout") as u64;
                    faults += (r.get("fault") == Some(&json!("module"))) as u64;
                }
            }
            let ones = |h: &Host| h.load().unwrap().maps.get("pending").map_or(0, |m| m.values().filter(|v| **v == 1).count());
            stuck_without += ones(&h); // before the last deadline passed
            for _ in 0..25 {
                h.call("reset_day", &[Value::Str("B".into())]).unwrap(); // let 25 ticks pass, no new requests
                h.flush().unwrap();
                for (_, _, r) in h.pump(vec![]).unwrap() {
                    timeouts += r.contains_key("timeout") as u64;
                }
            }
            stuck += ones(&h);
            mismatches += (!h.replay().unwrap().is_empty()) as u64;
        }
        eprintln!(
            "1000 runs, {calls} calls: {timeouts} timeout calls, {faults} invariant faults, {mismatches} replay mismatches; \
             accounts pending: {stuck_without} right after the run, {stuck} once every deadline has passed"
        );
        assert_eq!((stuck, faults, mismatches), (0, 0, 0));
    }

    #[test]
    fn a_record_parameter_is_one_json_object() {
        let params: Vec<(String, String)> =
            [("who", "Account"), ("p.x", "Int"), ("p.y", "Int"), ("n", "Int")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let got = http_args(&params, &json!({"n": 3, "p": {"y": 2, "x": 1}, "who": "A"})).unwrap();
        assert_eq!(got, vec![Value::Str("A".into()), Value::Int(1), Value::Int(2), Value::Int(3)]);
        let want = "want a JSON object with exactly ['who', 'p', 'n']; 'p' an object with exactly ['x', 'y']";
        for bad in [
            json!({"who": "A", "p.x": 1, "p.y": 2, "n": 3}), // the flattened names are not the interface
            json!({"who": "A", "p": {"x": 1}, "n": 3}),
            json!({"who": "A", "p": {"x": 1, "y": 2, "z": 0}, "n": 3}),
            json!({"who": "A", "p": [1, 2], "n": 3}),
            json!({"who": "A", "p": {"x": 1, "y": 2}}),
        ] {
            assert_eq!(http_args(&params, &bad).unwrap_err(), want, "{bad}");
        }
        // a nested record is an object inside the record's object
        let nested: Vec<(String, String)> =
            ["l.a.x", "l.a.y", "l.b.x", "l.b.y", "k"].iter().map(|a| (a.to_string(), "Int".to_string())).collect();
        let body = json!({"k": 5, "l": {"b": {"x": 3, "y": 4}, "a": {"x": 1, "y": 2}}});
        assert_eq!(http_args(&nested, &body).unwrap(), [1, 2, 3, 4, 5].map(Value::Int));
        let want = "want a JSON object with exactly ['l', 'k']; 'l' an object with exactly ['a', 'b']; \
                    'l.a' an object with exactly ['x', 'y']; 'l.b' an object with exactly ['x', 'y']";
        for bad in [
            json!({"k": 5, "l": {"a": {"x": 1, "y": 2}, "b": {"x": 3}}}),
            json!({"k": 5, "l": {"a": {"x": 1, "y": 2}, "b.x": 3, "b.y": 4}}),
            json!({"k": 5, "l": {"a": 1, "b": {"x": 3, "y": 4}}}),
        ] {
            assert_eq!(http_args(&nested, &bad).unwrap_err(), want, "{bad}");
        }
        // no records: the message as before
        let plain = vec![("a".to_string(), "Account".to_string())];
        assert_eq!(http_args(&plain, &json!({})).unwrap_err(), "want a JSON object with exactly ['a']");
    }

    #[test]
    fn a_record_call_over_http() {
        let src = "record Point x:Int y:Int\nstate at Int\n\nplace p:Point\n  need p.x >= 0 else Neg\n  ensure at.get' p.x = p.y\nbody\n  at.set p.x p.y\n";
        let defs = crate::syntax::parse_module(src, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let o = Options { sinks: Some(vec![]), gate: false, ..Default::default() };
        let mut h = Host::new(&defs, IndexMap::new(), o).unwrap();
        h.init("").unwrap();
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", srv.server_addr().to_ip().unwrap().port());
        let client = std::thread::spawn(move || {
            let post = |b: &str| match ureq::post(&format!("{base}/place")).send_string(b) {
                Ok(r) => (r.status(), r.into_string().unwrap()),
                Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap()),
                Err(e) => panic!("{e}"),
            };
            (post(r#"{"p": {"x": 4, "y": 9}}"#), post(r#"{"p": {"x": -1, "y": 9}}"#), post(r#"{"p.x": 4, "p.y": 9}"#))
        });
        for _ in 0..3 {
            handle_http(&mut h, srv.recv().unwrap()).unwrap();
        }
        let (ok, refused, bad) = client.join().unwrap();
        assert_eq!((ok.0, refused.0, bad.0), (200, 409, 400), "{ok:?} {refused:?} {bad:?}");
        assert!(refused.1.contains("Neg"));
        assert_eq!(h.load().unwrap().maps["at"][&Key::Int(4)], 9);
    }

    const WIDE: &str = "state hold Int

put x:Int k:Int
  ok otherwise
  ensure hold.get' (x + k) = 1
body
  hold.set (x + k) 1

store x:Int k:Int
  ok otherwise
  ensure hold.get' 0 = x + k
body
  hold.set 0 (x + k)

tell x:Int k:Int
  ok otherwise
  emit Sum (x + k)
body
  audit.emit (Sum (x + k))

nope x:Int k:Int
  need x < 0 else Big (x + k)
  ok otherwise
";

    fn wide() -> Module {
        let m = crate::syntax::parse_module(WIDE, true).unwrap();
        crate::wf::check_module(&m).unwrap();
        m
    }

    #[test]
    fn an_int_past_i64_is_a_host_fault_not_a_wrapped_value() {
        // S20: the interpreter computes in i128, the host stores i64
        let defs = wide();
        let mut h = Host::new(&defs, IndexMap::new(), Options { sinks: Some(vec![]), gate: false, ..Default::default() }).unwrap();
        h.init("").unwrap();
        let max = Value::Int(i64::MAX as Int);
        for (name, what) in [("put", "hold.9223372036854775808 = 1"), ("store", "hold.0 = 9223372036854775808"), ("tell", "event"), ("nope", "refusal Big")] {
            let r = h.call(name, &[max.clone(), Value::Int(1)]).unwrap();
            assert_eq!((r["status"].as_str(), r["fault"].as_str()), (Some("error"), Some("range")), "{name}: {r:?}");
            assert!(r["error"].as_str().unwrap().contains(what), "{name}: {r:?}");
        }
        // in range, the same definitions work
        assert_eq!(h.call("put", &[max.clone(), Value::Int(0)]).unwrap()["status"], "ok");
        assert_eq!(h.call("nope", &[max.clone(), Value::Int(-1)]).unwrap()["refused"], "Big");
        assert_eq!(h.flush().unwrap(), 0, "nothing out of range reached the outbox");
        assert_eq!(h.load().unwrap().maps["hold"], IndexMap::from([(Key::Int(i64::MAX as Int), 1)]));
        assert!(h.replay().unwrap().is_empty(), "{:?}", h.replay());
        // input: a number that isn't an i64 never becomes an Int
        assert_eq!(json_value(&json!(1.5)), Value::Str("1.5".into()));
        assert_eq!(json_value(&json!(9223372036854775808u64)), Value::Str("9223372036854775808 (outside i64)".into()));
        let below: J = serde_json::from_str("-9223372036854775809").unwrap();
        assert_eq!(json_value(&below), Value::Str("-9.223372036854776e18 (outside i64)".into()));
        assert_eq!(parse_args(&["9223372036854775808", "-5"]), vec![Value::Str("9223372036854775808".into()), Value::Int(-5)]);
        let e = Host::new(&defs, IndexMap::new(), Options { sinks: Some(vec![]), gate: false, ..Default::default() })
            .unwrap()
            .init("hold.1:9223372036854775808")
            .unwrap_err();
        // refused: by the parser since 7cbcd28 (literals outside i64), else by the host's own check
        assert!(e.contains("outside Int's range") || e.contains("out of the host's range"), "{e}");
    }

    #[test]
    fn a_fractional_int_over_http_is_a_bad_call() {
        let defs = wide();
        let mut h = Host::new(&defs, IndexMap::new(), Options { sinks: Some(vec![]), gate: false, ..Default::default() }).unwrap();
        h.init("").unwrap();
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", srv.server_addr().to_ip().unwrap().port());
        let client = std::thread::spawn(move || {
            ["{\"x\": 1.5, \"k\": 1}", "{\"x\": 9223372036854775808, \"k\": 1}", "{\"x\": 9223372036854775807, \"k\": 1}"]
                .map(|b| match ureq::post(&format!("{base}/store")).send_string(b) {
                    Ok(r) => r.status(),
                    Err(ureq::Error::Status(c, _)) => c,
                    Err(e) => panic!("{e}"),
                })
        });
        for _ in 0..3 {
            handle_http(&mut h, srv.recv().unwrap()).unwrap();
        }
        // 1.5 and 2^63 are not Ints (400); i64::MAX + 1 computed inside is a host fault (500)
        assert_eq!(client.join().unwrap(), [400, 400, 500]);
        assert!(h.load().unwrap().maps.get("hold").map_or(true, |m| m.is_empty()));
    }

    #[test]
    fn the_host_page_shows_the_modules_why() {
        let dir = std::env::temp_dir().join(format!("kern-hostpage-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let k = dir.join("w.k");
        std::fs::write(&k, "state hold Int\n\nput k:Int\n  ok otherwise\n  ensure hold.get' k = 1\nbody\n  hold.set k 1\n").unwrap();
        let defs = crate::syntax::parse_module(&std::fs::read_to_string(&k).unwrap(), true).unwrap();
        let anchor = crate::why::anchor_hash(&defs, &defs.defs["put"]);
        std::fs::write(dir.join("w.why"), format!("put@{anchor}\n  one flag per key, so a retry is harmless\n")).unwrap();
        let html = dir.join("p.html");
        let argv: Vec<String> =
            ["page", &k.to_string_lossy(), "--html", &html.to_string_lossy()].iter().map(|s| s.to_string()).collect();
        let analyse = |d: &Module, m: bool| crate::report::analyse(d, m, None, None).map_err(|e| e.message());
        assert_eq!(main(&argv, &analyse), 0);
        let page = std::fs::read_to_string(&html).unwrap();
        assert!(page.contains("one flag per key, so a retry is harmless"));
        // --why names another file, as for kern check
        let other = dir.join("other.why");
        std::fs::write(&other, format!("put@{anchor}\n  read from --why\n")).unwrap();
        let mut argv2 = argv.clone();
        argv2.extend(["--why".to_string(), other.to_string_lossy().into_owned()]);
        assert_eq!(main(&argv2, &analyse), 0);
        let page = std::fs::read_to_string(&html).unwrap();
        assert!(page.contains("read from --why") && !page.contains("one flag per key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_huge_bound_ends_as_a_fault_not_a_held_host() {
        // S21: the call's iterations are charged when a loop or sum starts, so this fails at once
        let src = "state hold Int\n\ncount n:Int\n  ok otherwise\nbody\n  hold.set 0 (sum j from 0 to n of 1)\n\n\
                   fill n:Int\n  ok otherwise\nbody\n  for k from 0 to n\n    hold.set 1 k\n";
        let defs = crate::syntax::parse_module(src, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let mut h = Host::new(&defs, IndexMap::new(), Options { sinks: Some(vec![]), gate: false, ..Default::default() }).unwrap();
        h.init("").unwrap();
        let t = std::time::Instant::now();
        for (name, n) in [("count", i64::MAX as Int), ("fill", 1_000_000_000_000)] {
            let r = h.call(name, &[Value::Int(n)]).unwrap();
            assert_eq!((r["status"].as_str(), r["fault"].as_str()), (Some("error"), Some("limit")), "{name}: {r:?}");
        }
        assert!(t.elapsed().as_secs() < 2, "{:?}", t.elapsed());
        assert_eq!(h.call("count", &[Value::Int(ITERATION_LIMIT)]).unwrap()["status"], "ok");
        assert_eq!(h.call("count", &[Value::Int(ITERATION_LIMIT + 1)]).unwrap()["fault"], "limit");
        assert_eq!(h.load().unwrap().maps["hold"][&Key::Int(0)], ITERATION_LIMIT);
        assert!(h.replay().unwrap().is_empty());
        assert!(manifest_text(&h).contains("iteration limit: 10000000 loop and sum iterations per call"));
        assert!(host_facts(&h).assumptions.iter().any(|a| a.contains(&format_num(ITERATION_LIMIT))));
        assert!(manifest_text(&h).contains("call depth: definitions nest at most 200 deep per call"));
        assert!(host_facts(&h).assumptions.iter().any(|a| a.starts_with("Definitions nest at most 200 deep")));
        // the compiled program stops at the same number
        let rt = include_str!("../../runtime/src/lib.rs");
        assert!(rt.contains(&format!("pub const ITERATION_LIMIT: i64 = {};", "10_000_000")) && ITERATION_LIMIT == 10_000_000);
    }

    const NOTES: &str = "state title Int -> Text
state size Int
channel net

post k:Int t:Text
  need length t > 0 else Empty
  ok otherwise
  ensure title.get' k = t + \"!\"
  emit Posted k t
body
  title.set k (t + \"!\")
  size.set k (length t)
  audit.emit (Posted k t)

ask k:Int q:Text
  ok otherwise
body
  net.emit (Lookup k q)

answer k:Int q:Text a:Text
  ok otherwise
body
  title.set k a

missed k:Int q:Text
  ok otherwise
body
  title.set k \"?\"
";

    #[test]
    fn text_state_is_stored_replayed_and_served() {
        let defs = crate::syntax::parse_module(NOTES, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let config = "Lookup -> GET http://127.0.0.1:9/q/{1}/{2}; reply answer {1} {2} $.text; fail missed {1} {2}";
        let eps = load_config(config, &defs).unwrap();
        assert_eq!(eps["Lookup"].reply_types, ["Int", "Text", "Text"]);
        // a reply field for a Text parameter is a JSON string; a number there is a failure
        let ev = Value::Ctor("Lookup".into(), vec![Value::Int(1), Value::Str("a b/c".into())]);
        assert_eq!(eps["Lookup"].args_for(&ev, Some(&json!({"text": "hi"})), "reply").unwrap()[2], Value::Str("hi".into()));
        assert!(eps["Lookup"].args_for(&ev, Some(&json!({"text": 5})), "reply").is_err());
        assert_eq!(eps["Lookup"].url_for(&ev), "http://127.0.0.1:9/q/1/a%20b%2Fc");

        let dir = std::env::temp_dir().join(format!("kern-text-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("t.db").to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&db);
        let q: Rc<std::cell::RefCell<Vec<(String, Vec<Value>)>>> = Default::default();
        let open = |q: &Rc<std::cell::RefCell<Vec<(String, Vec<Value>)>>>| {
            let o = Options {
                db: db.clone(),
                sinks: Some(vec![]),
                gate: false,
                endpoints: Some(load_config(config, &defs).unwrap()),
                net: Some(Box::new(Scripted(q.clone()))),
                ..Default::default()
            };
            Host::new(&defs, IndexMap::new(), o).unwrap()
        };
        {
            let mut h = open(&q);
            assert!(h.init("title.9:\"from \\\"genesis\\\"\"").unwrap());
            let mut out = vec![];
            let lines = ["post 1 \"hello world\"", "post 2 \"\"", "ask 3 \"why?\""].iter().map(|l| l.to_string());
            q.borrow_mut().push(("answer".into(), vec![Value::Int(3), Value::Str("why?".into()), Value::Str("because".into())]));
            repl(&mut h, lines, &mut |l| out.push(l)).unwrap();
            assert_eq!(out, ["ok", "refused Empty", "ok", "reply answer 3 \"why?\" \"because\" -> ok"]);
            // a Text from the network may hold a line break: the reply still prints as one line
            assert_eq!(line_quote("a \"b\" \\n\nc\r"), r#""a \"b\" \\n\nc\r""#);
        }
        // a new process on the same database
        let mut h = open(&q);
        let w = h.load().unwrap();
        let t = |k: Int| w.texts["title"].get(&Key::Int(k)).cloned();
        assert_eq!((t(9), t(1), t(2), t(3)), (Some("from \"genesis\"".into()), Some("hello world!".into()), None, Some("because".into())));
        assert_eq!(w.maps["size"][&Key::Int(1)], 11);
        assert!(h.replay().unwrap().is_empty(), "{:?}", h.replay());
        // HTTP: a Text parameter is a JSON string; GET /state shows Text maps with string values
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", srv.server_addr().to_ip().unwrap().port());
        let client = std::thread::spawn(move || {
            let post = |b: &str| match ureq::post(&format!("{base}/post")).send_string(b) {
                Ok(r) => r.status(),
                Err(ureq::Error::Status(c, _)) => c,
                Err(e) => panic!("{e}"),
            };
            let codes = (post(r#"{"k": 4, "t": "ünïcode"}"#), post(r#"{"k": 4, "t": 5}"#));
            let state: J = serde_json::from_str(&ureq::get(&format!("{base}/state")).call().unwrap().into_string().unwrap()).unwrap();
            (codes, state)
        });
        for _ in 0..3 {
            handle_http(&mut h, srv.recv().unwrap()).unwrap();
        }
        let ((ok, bad), state) = client.join().unwrap();
        assert_eq!((ok, bad), (200, 400));
        assert_eq!(state["state"]["title"]["4"], "ünïcode!");
        assert!(h.replay().unwrap().is_empty());
        // a tampered Text cell is caught by replay
        h.db.execute("update textcell set value='x' where key='1'", []).unwrap();
        assert!(h.replay().unwrap().iter().any(|m| m.starts_with("text state differs")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_definition_that_writes_and_returns_gives_its_value_and_stays_atomic() {
        let src = "state next Int\nstate stock Int\n\ntake item:Int -> Int\n  need stock.get item > 0 else Empty\n  \
                   ok otherwise\n  ensure result = next.get 0 + 1\nbody\n  id = next.get 0 + 1\n  next.set 0 id\n  \
                   stock.set item (stock.get item - 1)\n  result = id\n\ntwo item:Int -> Int\nbody\n  a = take item\n  \
                   b = take item\n  need b < 4 else Many\n  result = a * 10 + b\n";
        let defs = crate::syntax::parse_module(src, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let dir = std::env::temp_dir().join(format!("kern-ret-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("t.db").to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&db);
        let open = || {
            let o = Options { db: db.clone(), sinks: Some(vec![]), gate: false, ..Default::default() };
            Host::new(&defs, IndexMap::new(), o).unwrap()
        };
        let mut h = open();
        h.init("stock.1:5").unwrap();
        let r = h.call("take", &[Value::Int(1)]).unwrap();
        assert_eq!((&r["status"], &r["result"]), (&json!("ok"), &json!(1)));
        assert_eq!(h.call("two", &[Value::Int(1)]).unwrap()["result"], json!(23));
        // the second take's write is undone with the refusal after it, the first one's too
        assert_eq!(h.call("two", &[Value::Int(1)]).unwrap()["refused"], json!("Many"));
        let w = h.load().unwrap();
        assert_eq!((w.maps["next"][&Key::Int(0)], w.maps["stock"][&Key::Int(1)]), (3, 2));
        let mut out = vec![];
        repl(&mut h, ["take 1", "two 1", "take 7"].iter().map(|l| l.to_string()), &mut |l| out.push(l)).unwrap();
        // one copy left after `take 1`: the second take in `two` refuses, and the first one's write goes with it
        assert_eq!(out, ["ok 4", "refused Empty", "refused Empty"]);
        let w = h.load().unwrap();
        assert_eq!((w.maps["next"][&Key::Int(0)], w.maps["stock"][&Key::Int(1)]), (4, 1));
        drop(h);
        assert!(open().replay().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn time_never_goes_back_after_a_call_that_ticked() {
        let src = "state at Int\n\nstamp k:Int\n  ok otherwise\nbody\n  at.set k clock.now\n  clock.tick\n  clock.tick\n  \
                   at.set (k + 1) clock.now\n\nmark k:Int\n  ok otherwise\nbody\n  at.set k clock.now\n";
        let defs = crate::syntax::parse_module(src, true).unwrap();
        crate::wf::check_module(&defs).unwrap();
        let dir = std::env::temp_dir().join(format!("kern-tick-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("t.db").to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&db);
        // a wall clock that stands still at 100
        let open = || {
            let o = Options { db: db.clone(), clock: Some(Box::new(|| 100)), sinks: Some(vec![]), gate: false, ..Default::default() };
            Host::new(&defs, IndexMap::new(), o).unwrap()
        };
        let mut h = open();
        h.init("").unwrap();
        let at = |h: &Host, k: Int| h.load().unwrap().maps["at"].get(&Key::Int(k)).copied();
        h.call("mark", &[Value::Int(0)]).unwrap();
        h.call("mark", &[Value::Int(1)]).unwrap();
        assert_eq!((at(&h, 0), at(&h, 1)), (Some(100), Some(100)), "no ticks: the wall clock as before");
        h.call("stamp", &[Value::Int(10)]).unwrap();
        assert_eq!((at(&h, 10), at(&h, 11)), (Some(100), Some(102)));
        h.call("mark", &[Value::Int(2)]).unwrap();
        assert_eq!(at(&h, 2), Some(103), "after the ticks, not the wall clock's 100");
        drop(h);
        let mut h = open(); // the floor survives a restart
        h.call("mark", &[Value::Int(3)]).unwrap();
        assert_eq!(at(&h, 3), Some(103));
        assert!(h.replay().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn requests_from_before_deadlines_are_not_timed_out() {
        let dir = std::env::temp_dir().join(format!("kern-s12-old-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("h.db").to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&db);
        let defs = module();
        {
            let mut h = host(&defs, &db, CONFIG, 100, Box::new(Scripted(Default::default())));
            h.call("request_limit", &[a()]).unwrap();
            h.flush().unwrap();
        }
        // make it a database from before the column
        let c = Connection::open(&db).unwrap();
        c.execute_batch("create table o2 as select seq, call, event, delivered, channel from outbox; drop table outbox; alter table o2 rename to outbox").unwrap();
        drop(c);
        let mut h = host(&defs, &db, CONFIG, 1000, Box::new(Scripted(Default::default())));
        assert!(tick(&mut h).is_empty());
        assert_eq!(pending(&h), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
