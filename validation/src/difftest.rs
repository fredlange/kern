//! Differential test (port of validation/difftest.py): a compiled Kern module against the reference interpreter.
//!
//! ```text
//! difftest [--per-def N] [--seed S] [MODULE.k ...]   (default: every example + tests/probes/{runtime,invcheck}.k)
//! ```
//!
//! Scorecard C1 and C2 of docs/research/RUNTIME_PLAN.md:
//! - C1: every `eg` line, run in the binary, gives the eg's outcome and after-world;
//! - C2: random worlds and calls (the same world, calls and provider values for both) give the same canonical line
//!   per call (outcome, refusal payload, clock, events per channel) and the same world.
//!
//! The reference is the interpreter plus the host's key-scoped module-invariant check, for the invariants
//! `kern build` compiles a check of (`kern::emit_rust::unproven_invariants`): a call that breaks one at a key it
//! wrote is a fault, undone, with no clock tick.
//!
//! Values stay small, so no interpreter integer leaves i64 (overflow is a deliberate divergence, tested separately
//! in kern/tests/runtime.rs). The random draws use the same Python-compatible generator and the same order as
//! the Python tool, so the scripts are the Python tool's scripts.

use indexmap::IndexMap;
use kern::pyrandom::Random;
use kern::run::{Interp, Outcome, RunError, Value, World};
use kern::syntax::{Example, Key, Module};

pub const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const NAMES: [&str; 6] = ["A", "B", "C", "D", "E", "H"];
const INTS: [i128; 14] = [-1, 0, 1, 2, 3, 5, 7, 50, 99, 100, 101, 150, 250, 1000];
const BALANCES: [i128; 9] = [0, 1, 5, 50, 99, 100, 101, 250, 1000];
const MAP_VALUES: [i128; 7] = [0, 1, 2, 5, 100, 250, -1];

/// A value the binary can print: a nested constructor isn't one.
fn show(v: &Value) -> Result<String, String> {
    match v {
        Value::Bool(b) => Ok(if *b { "True" } else { "False" }.into()),
        Value::Int(n) => Ok(n.to_string()),
        Value::Str(s) => Ok(s.clone()),
        Value::None => Ok("None".into()),
        Value::Ctor(..) => Err(format!("value {} (nested constructor)", v.repr())),
    }
}

fn show_event(ev: &Value) -> Result<String, String> {
    match ev {
        Value::Ctor(c, a) => {
            let mut parts = vec![c.clone()];
            for x in a {
                parts.push(show(x)?);
            }
            Ok(parts.join(" "))
        }
        other => show(other),
    }
}

fn key_str(k: &Key) -> String {
    match k {
        Key::Int(n) => n.to_string(),
        Key::Str(s) => s.clone(),
    }
}

fn world_line(bal: &IndexMap<String, i128>, own: &IndexMap<String, String>, maps: &IndexMap<String, IndexMap<Key, i128>>, now: i128) -> String {
    let mut items: Vec<String> = bal.iter().map(|(a, b)| format!("{}:{}@{}", a, b, own[a])).collect();
    for (m, kv) in maps {
        items.extend(kv.iter().map(|(k, v)| format!("{}.{}:{}", m, key_str(k), v)));
    }
    items.push(format!("now:{}", now));
    format!("world {}", items.join(" "))
}

fn dump(w: &World, defs: &Module) -> String {
    let mut s = format!("clock={} |", w.clock);
    for (a, b) in &w.bal {
        s.push_str(&format!(" {}:{}@{}", a, b, w.own[a]));
    }
    s.push_str(" |");
    let mut states: Vec<&String> = defs.states.keys().collect();
    states.sort();
    for m in states {
        let mut kv: Vec<(&Key, &i128)> = w.maps.get(m).map(|kv| kv.iter().filter(|(_, v)| **v != 0).collect()).unwrap_or_default();
        if defs.states[m] == "Account" {
            kv.sort_by_key(|(k, _)| key_str(k));
        } else {
            kv.sort_by_key(|(k, _)| match k {
                Key::Int(n) => *n,
                Key::Str(_) => 0,
            });
        }
        for (k, v) in kv {
            s.push_str(&format!(" {}.{}:{}", m, key_str(k), v));
        }
    }
    s
}

fn channels(defs: &Module) -> Vec<String> {
    let mut c = vec!["audit".to_string()];
    c.extend(defs.channels.iter().cloned());
    c
}

/// A world as a read-only Store, so a checked call runs on `World::lazy` and its written keys are exact, as in the
/// host (`kern host`'s call_in_tx).
struct Snap(World);

impl kern::run::Store for Snap {
    fn balance(&self, a: &str) -> Option<i128> {
        self.0.bal.get(a).copied()
    }
    fn owner(&self, a: &str) -> Option<String> {
        self.0.own.get(a).cloned()
    }
    fn total(&self) -> i128 {
        self.0.bal.values().sum()
    }
    fn cell(&self, state: &str, key: &Key) -> Option<i128> {
        self.0.maps.get(state).and_then(|m| m.get(key)).copied()
    }
}

/// Module invariants the compiled program checks after a call (`kern::emit_rust::unproven_invariants`): name ->
/// indexes into `Module::invariants`.
pub type Checks = IndexMap<String, Vec<usize>>;

/// One call through the reference: the interpreter, plus, for a definition in `checks`, the host's key-scoped
/// invariant check (only the invariants the compiled program checks): a successful call that breaks one at a key
/// it wrote is undone as a fault and doesn't tick the clock. Returns (canonical line, provider values it drew).
fn interp_call(interp: &mut Interp, w: &mut World, defs: &Module, checks: &Checks, name: &str, args: &[Value])
    -> Result<(String, Vec<Value>), String> {
    let Some(which) = checks.get(name) else { return interp_call_plain(interp, w, defs, name, args) };
    let mut lw = World::lazy(std::rc::Rc::new(Snap(w.clone())));
    lw.clock = w.clock;
    let n_obs = interp.observations.len();
    let res = interp_call_plain(interp, &mut lw, defs, name, args)?;
    if res.0.starts_with("ok") {
        let keys = lw.written_keys(&defs.states);
        let texts: Vec<String> =
            which.iter().map(|&i| { let (v, t, c) = &defs.invariants[i]; format!("{}:{} {}", v, t, kern::syntax::code(c)) }).collect();
        let broken = interp.broken_invariants(&mut lw, Some(&keys)).map_err(|e| format!("interpreter: {e}"))?;
        if broken.iter().any(|(t, _)| texts.contains(t)) {
            let drawn = interp.observations[n_obs..].iter().map(|(_, _, v)| v.clone()).collect::<Vec<_>>();
            return Ok((format!("fault t={}", w.clock), drawn));
        }
    }
    // keep what the call did: its writes, events and clock
    w.bal.extend(lw.bal);
    w.own.extend(lw.own);
    for (x, m) in lw.maps {
        w.maps.entry(x).or_default().extend(m);
    }
    w.audit.extend(lw.audit);
    for (c, evs) in lw.out {
        w.out.entry(c).or_default().extend(evs);
    }
    w.clock = lw.clock;
    Ok(res)
}

fn interp_call_plain(interp: &mut Interp, w: &mut World, defs: &Module, name: &str, args: &[Value]) -> Result<(String, Vec<Value>), String> {
    let d = &defs.defs[name];
    let before: Vec<usize> = channels(defs).iter().map(|c| w.events(c).len()).collect();
    let n_obs = interp.observations.len();
    let drawn = |interp: &Interp| interp.observations[n_obs..].iter().map(|(_, _, v)| v.clone()).collect::<Vec<_>>();
    let outcome = match interp.run(name, args, w) {
        Err(RunError::Kern(e)) => {
            let kind = if e.starts_with("bad call") { "badcall" } else { "fault" };
            return Ok((format!("{} t={}", kind, w.clock), drawn(interp)));
        }
        Err(e) => return Err(format!("interpreter: {e}")),
        Ok(o) => o,
    };
    let mut line = match &outcome {
        Outcome::Ok if d.ret.is_some() => format!("ok result={}", interp.last_result.as_ref().map(|v| v.str()).unwrap_or("None".into())),
        Outcome::Ok => "ok".into(),
        Outcome::Fail(c, a) => {
            let mut parts = vec!["refused".to_string(), c.clone()];
            for x in a {
                parts.push(show(x)?);
            }
            parts.join(" ")
        }
    };
    line.push_str(&format!(" t={}", w.clock));
    for (i, c) in channels(defs).iter().enumerate() {
        let new = &w.events(c)[before[i]..];
        if !new.is_empty() {
            let evs: Result<Vec<String>, String> = new.iter().map(show_event).collect();
            line.push_str(&format!(" | {}: {}", c, evs?.join(", ")));
        }
    }
    Ok((line, drawn(interp)))
}

/// A call's tokens as the interpreter's arguments (Int parameters parse; extra tokens stay strings).
fn interp_args(d: &kern::syntax::Def, toks: &[String]) -> Vec<Value> {
    let mut out: Vec<Value> = d
        .params
        .iter()
        .zip(toks)
        .map(|((_, t), tok)| if t == "Int" { tok.parse().map(Value::Int).unwrap_or(Value::Str(tok.clone())) } else { Value::Str(tok.clone()) })
        .collect();
    out.extend(toks[d.params.len().min(toks.len())..].iter().map(|t| Value::Str(t.clone())));
    out
}

/// A script for the binary plus what the interpreter said, line by line.
#[derive(Default)]
pub struct Case {
    pub script: Vec<String>,
    pub expected: Vec<String>,
    pub place: Vec<String>,
}

impl Case {
    fn call(&mut self, ctx: String, toks: &[String], line: String, obs: &[Value]) -> Result<(), String> {
        if !obs.is_empty() {
            let vals: Result<Vec<String>, String> = obs.iter().map(show).collect();
            self.script.push(format!("obs {}", vals?.join(" ")));
        }
        self.script.push(format!("call {}", toks.join(" ")));
        self.expected.push(line);
        self.place.push(ctx);
        Ok(())
    }
    fn dump(&mut self, ctx: String, d: String) {
        self.script.push("dump".into());
        self.expected.push(d);
        self.place.push(ctx);
    }
}

/// Python's random.sample(population, k).
fn sample<T: Clone>(rng: &mut Random, pop: &[T], k: usize) -> Vec<T> {
    let n = pop.len();
    let mut result: Vec<Option<T>> = vec![None; k];
    // CPython 3.9: the pool algorithm when n <= setsize (21 + 4^ceil(log4(3k)) for small k)
    let mut setsize = 21usize;
    if k > 5 {
        setsize += 4usize.pow(((k * 3) as f64).log(4.0).ceil() as u32);
    }
    if n <= setsize {
        let mut pool: Vec<T> = pop.to_vec();
        for (i, slot) in result.iter_mut().enumerate() {
            let j = rng.randbelow((n - i) as u128) as usize;
            *slot = Some(pool[j].clone());
            pool[j] = pool[n - i - 1].clone();
        }
    } else {
        let mut selected = std::collections::BTreeSet::new();
        for slot in result.iter_mut() {
            let mut j = rng.randbelow(n as u128) as usize;
            while selected.contains(&j) {
                j = rng.randbelow(n as u128) as usize;
            }
            selected.insert(j);
            *slot = Some(pop[j].clone());
        }
    }
    result.into_iter().map(Option::unwrap).collect()
}

type Maps = IndexMap<String, IndexMap<Key, i128>>;

fn random_world(rng: &mut Random, defs: &Module) -> (IndexMap<String, i128>, IndexMap<String, String>, Maps, i128) {
    let k = rng.randint(2, 5) as usize;
    let names = sample(rng, &NAMES, k);
    let bal: IndexMap<String, i128> = names.iter().map(|a| (a.to_string(), *rng.choice(&BALANCES))).collect();
    let own: IndexMap<String, String> = names.iter().map(|a| (a.to_string(), format!("u{}", rng.randint(1, 3)))).collect();
    let mut maps = Maps::new();
    for (m, key) in &defs.states {
        let mut kv = IndexMap::new();
        for _ in 0..rng.randint(0, 3) {
            let k = if key == "Account" { Key::Str(rng.choice(&names).to_string()) } else { Key::Int(*rng.choice(&[0, 1, 2, 3, 7])) };
            kv.insert(k, *rng.choice(&MAP_VALUES));
        }
        if !kv.is_empty() {
            maps.insert(m.clone(), kv);
        }
    }
    (bal, own, maps, rng.randint(0, 20) as i128)
}

fn tok_of(k: &Key) -> String {
    key_str(k)
}

/// The script and the interpreter's answers for one module. `provider` serves assumed handlers (None: the
/// interpreter's seeded default); whatever it draws goes into the script as `obs` lines for the binary to replay.
pub fn build_case<'m>(defs: &'m Module, checks: &Checks, rng: &mut Random, per_def: usize, seed: i128, provider: Option<kern::run::Provider<'m>>)
    -> Result<(Case, Vec<(usize, Example)>), String> {
    let mut case = Case::default();
    let mut interp = Interp::with_provider(defs, provider, seed);
    // the module's own examples first (C1), then random worlds (C2)
    let mut egs = vec![];
    for d in defs.defs.values() {
        for ex in &d.examples {
            let bal: IndexMap<String, i128> = ex.world.iter().map(|(a, (b, _))| (a.clone(), *b)).collect();
            let own: IndexMap<String, String> = ex.world.iter().map(|(a, (_, o))| (a.clone(), o.clone())).collect();
            case.script.push(world_line(&bal, &own, &ex.maps, ex.now));
            let mut w = World::from_example(&ex.world, ex.now, &ex.maps);
            let (name, toks) = (&ex.call.0, ex.call.1.iter().map(tok_of).collect::<Vec<_>>());
            let Some(cd) = defs.defs.get(name) else { continue };
            let (line, obs) = interp_call(&mut interp, &mut w, defs, checks, name, &interp_args(cd, &toks))?;
            let ctx = format!("eg {}", ex.src);
            let mut all = vec![name.clone()];
            all.extend(toks);
            case.call(ctx.clone(), &all, line, &obs)?;
            case.dump(ctx, dump(&w, defs));
            egs.push((case.expected.len() - 2, ex.clone()));
        }
    }
    let names: Vec<&String> = defs.defs.keys().collect();
    let n_calls = per_def * names.len();
    let mut done = 0;
    while done < n_calls {
        let (bal, own, maps, now) = random_world(rng, defs);
        let wl = world_line(&bal, &own, &maps, now);
        case.script.push(wl.clone());
        let mut w = World { bal: bal.clone(), own: own.clone(), clock: now, maps: maps.clone(), ..Default::default() };
        let accounts: Vec<String> = bal.keys().cloned().collect();
        for _ in 0..15 {
            let name = if done < names.len() * per_def / 2 { names[done % names.len()] } else { *rng.choice(&names) };
            let d = &defs.defs[name];
            let toks: Vec<String> = d
                .params
                .iter()
                .map(|(_, t)| {
                    if t == "Account" {
                        if rng.random() > 0.03 {
                            rng.choice(&accounts).clone()
                        } else {
                            "Z".into()
                        }
                    } else {
                        rng.choice(&INTS).to_string()
                    }
                })
                .collect();
            let (line, obs) = interp_call(&mut interp, &mut w, defs, checks, name, &interp_args(d, &toks))?;
            let mut all = vec![name.clone()];
            all.extend(toks.iter().cloned());
            case.call(format!("{} | {} {}", wl, name, toks.join(" ")), &all, line, &obs)?;
            done += 1;
        }
        case.dump(wl, dump(&w, defs));
    }
    Ok((case, egs))
}

/// C1: the binary's own output against the eg line (not against the interpreter).
fn eg_problems(ex: &Example, line: &str, world_dump: &str) -> Vec<String> {
    let want = match &ex.outcome {
        kern::syntax::Outcome::Ok => "ok".to_string(),
        kern::syntax::Outcome::Fail(c, a) => {
            let mut parts = vec!["refused".to_string(), c.clone()];
            parts.extend(a.iter().map(|x| x.to_string()));
            parts.join(" ")
        }
    };
    let got = line.split(" t=").next().unwrap_or("");
    let mut out = vec![];
    if !(got == want || (want == "ok" && got.starts_with("ok"))) {
        out.push(format!("outcome {:?}, eg says {:?}", got, want));
    }
    let items: Vec<&str> = world_dump.split_whitespace().collect();
    for (a, b) in &ex.after {
        if !items.iter().any(|i| i.starts_with(&format!("{}:{}@", a, b))) {
            out.push(format!("{} should be {}: {}", a, b, world_dump));
        }
    }
    for (m, kv) in &ex.after_maps {
        for (k, v) in kv {
            let item = format!("{}.{}:{}", m, key_str(k), v);
            let prefix = format!("{}.{}:", m, key_str(k));
            if (*v != 0 && !items.contains(&item.as_str())) || (*v == 0 && items.iter().any(|i| i.starts_with(&prefix))) {
                out.push(format!("{}.{} should be {}: {}", m, key_str(k), v, world_dump));
            }
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct ModuleResult {
    pub module: String,
    pub defs: usize,
    pub calls: usize,
    pub egs: usize,
    pub eg_failures: Vec<String>,
    /// (line, where, interpreter, binary), at most `show_max`
    pub divergences: Vec<(usize, String, String, String)>,
    pub n_divergences: usize,
    pub interp_s: f64,
    pub binary_s: f64,
}

/// Why a module wasn't compared: the emitter doesn't support it (skipped), or something failed (a test failure:
/// a generated crate that doesn't build, a binary that crashes, an interpreter error).
#[derive(Debug)]
pub enum ModuleError {
    Unsupported(String),
    Failed(String),
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ModuleError::Unsupported(m) => write!(f, "UNSUPPORTED {m}"),
            ModuleError::Failed(m) => write!(f, "FAILED {m}"),
        }
    }
}

impl From<String> for ModuleError {
    fn from(m: String) -> Self {
        ModuleError::Failed(m)
    }
}

impl From<kern::emit_rust::BuildError> for ModuleError {
    fn from(e: kern::emit_rust::BuildError) -> Self {
        match e {
            kern::emit_rust::BuildError::Unsupported(m) => ModuleError::Unsupported(m),
            other => ModuleError::Failed(other.to_string()),
        }
    }
}

/// Build the module, run the same script through both sides and compare.
pub fn run_module(path: &str, per_def: usize, seed: i128, show_max: usize) -> Result<ModuleResult, ModuleError> {
    let defs = kern::package::load(path, &IndexMap::new()).map_err(|e| ModuleError::Failed(e.0))?;
    let crate_dir = kern::emit_rust::emit(path, None)?;
    // what `kern build` compiled in: the same list emit() computed
    let checks = kern::emit_rust::unproven_invariants(&defs)?;
    let binary = kern::emit_rust::build(&crate_dir)?;
    let mut rng = Random::from_int(seed);
    let t0 = std::time::Instant::now();
    // build_case fails on a value the binary can't print (a nested constructor: unsupported, as in Python's
    // difftest) or on an interpreter error (a failure)
    let (case, egs) = build_case(&defs, &checks, &mut rng, per_def, seed, None)
        .map_err(|e| if e.starts_with("interpreter: ") { ModuleError::Failed(e) } else { ModuleError::Unsupported(e) })?;
    let interp_s = t0.elapsed().as_secs_f64();
    let script = std::env::temp_dir().join(format!("kern-difftest-{}-{}.script", std::process::id(), kern::emit_rust::crate_name(path)));
    std::fs::write(&script, case.script.join("\n") + "\n").map_err(|e| e.to_string())?;
    let t0 = std::time::Instant::now();
    let r = std::process::Command::new(&binary).arg("run").arg(&script).output().map_err(|e| e.to_string())?;
    let binary_s = t0.elapsed().as_secs_f64();
    let _ = std::fs::remove_file(&script);
    if !r.status.success() {
        return Err(format!("{} failed: {}", binary, String::from_utf8_lossy(&r.stderr)).into());
    }
    let got: Vec<String> = String::from_utf8_lossy(&r.stdout).lines().map(String::from).collect();
    let n = got.len().max(case.expected.len());
    let div: Vec<(usize, String, String, String)> = (0..n)
        .filter(|&i| i >= case.expected.len() || i >= got.len() || got[i] != case.expected[i])
        .map(|i| {
            (
                i,
                case.place.get(i).cloned().unwrap_or_default(),
                case.expected.get(i).cloned().unwrap_or_default(),
                got.get(i).cloned().unwrap_or("<missing>".into()),
            )
        })
        .collect();
    let mut eg_failures = vec![];
    for (i, ex) in &egs {
        for p in eg_problems(ex, &got[*i], &got[*i + 1]) {
            eg_failures.push(format!("{}: {}", ex.src, p));
        }
    }
    Ok(ModuleResult {
        module: path.rsplit('/').next().unwrap_or(path).to_string(),
        defs: defs.defs.len(),
        calls: case.script.iter().filter(|s| s.starts_with("call ")).count(),
        egs: egs.len(),
        eg_failures,
        n_divergences: div.len(),
        divergences: div.into_iter().take(show_max).collect(),
        interp_s,
        binary_s,
    })
}

/// Every example: a package directory as one module, otherwise each .k file of the project.
pub fn example_modules() -> Vec<String> {
    let mut dirs: Vec<std::path::PathBuf> =
        std::fs::read_dir(format!("{ROOT}/examples")).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    let mut out = vec![];
    for d in dirs {
        if d.join("kern.pkg").exists() {
            out.push(d.to_string_lossy().into_owned());
        } else {
            let mut ks: Vec<String> = std::fs::read_dir(&d)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map_or(false, |x| x == "k"))
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            ks.sort();
            out.extend(ks);
        }
    }
    out
}

pub fn main(argv: &[String]) -> i32 {
    let (mut per_def, mut seed, mut mods) = (2000usize, 1i128, vec![]);
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--per-def" => {
                i += 1;
                per_def = argv[i].parse().unwrap();
            }
            "--seed" => {
                i += 1;
                seed = argv[i].parse().unwrap();
            }
            m => mods.push(m.to_string()),
        }
        i += 1;
    }
    if mods.is_empty() {
        mods = example_modules();
        mods.push(format!("{ROOT}/tests/probes/runtime.k")); // paths the examples never reach
        mods.push(format!("{ROOT}/tests/probes/invcheck.k")); // compiled module-invariant checks that break
    }
    let (mut calls, mut divs, mut eg_fail, mut egs, mut failed) = (0, 0, 0, 0, 0usize);
    for m in &mods {
        let res = match run_module(m, per_def, seed, 5) {
            Ok(r) => r,
            Err(e) => {
                println!("{:18} {}", m.rsplit('/').next().unwrap_or(m), e);
                failed += matches!(e, ModuleError::Failed(_)) as usize;
                continue;
            }
        };
        calls += res.calls;
        divs += res.n_divergences;
        eg_fail += res.eg_failures.len();
        egs += res.egs;
        println!(
            "{:18} defs={:3} calls={:7} egs={:3} eg_failures={} divergences={} (interpreter {:.1}s, binary {:.2}s)",
            res.module,
            res.defs,
            res.calls,
            res.egs,
            res.eg_failures.len(),
            res.n_divergences,
            res.interp_s,
            res.binary_s
        );
        for f in res.eg_failures.iter().take(5) {
            println!("   EG   {}", f);
        }
        for (i, place, want, got) in &res.divergences {
            println!("   DIV line {}: {}\n        interpreter: {}\n        binary:      {}", i, place, want, got);
        }
    }
    println!("TOTAL calls={} egs={} eg_failures={} divergences={} failed_modules={}", calls, egs, eg_fail, divs, failed);
    if divs == 0 && eg_fail == 0 && failed == 0 {
        0
    } else {
        1
    }
}
