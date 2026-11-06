//! The R1 driver: `defs`, `run [SCRIPT]` (the differential-test protocol) and `bench`.
//!
//! Script lines (one command per line):
//!   world ITEMS...        reset the world: `A:500@u1`, `hold.7:100`, `frozen.A:1`, `title.2:"a b"`, `now:3`
//!   obs V...              values the provider hands out next (recorded observations)
//!   call DEF ARG...       one atomic call; prints one canonical line (a Text argument is quoted: `"a \"b\""`)
//!   dump                  prints the world, canonically
//! The Python harness (validation/difftest.py) prints the same lines from kern/run.py.
use crate::*;
use std::io::{BufRead, Write};
use std::time::Instant;

pub fn main<M: Module>() {
    main_with::<M>(std::env::args().collect())
}

/// `main` with explicit arguments (argv[0] is the program name), so one binary can host many modules.
pub fn main_with<M: Module>(argv: Vec<String>) {
    match argv.get(1).map(|s| s.as_str()) {
        Some("defs") => {
            for d in M::DEFS {
                let ps: Vec<String> = d.params.iter().map(|(p, t)| format!("{p}:{t:?}")).collect();
                println!("{} {} {}{} uses {}", d.hash, d.name, ps.join(" "), d.ret.map(|r| format!(" -> {r}")).unwrap_or_default(), d.uses.join(" "));
            }
        }
        Some("run") => {
            let input: Box<dyn BufRead> = match argv.get(2) {
                Some(f) => Box::new(std::io::BufReader::new(std::fs::File::open(f).expect("script"))),
                None => Box::new(std::io::BufReader::new(std::io::stdin())),
            };
            let out = std::io::stdout();
            let mut out = std::io::BufWriter::new(out.lock());
            run_script::<M>(input, &mut out);
        }
        Some("bench") => {
            let def = argv.get(2).expect("bench DEF ACCOUNTS CALLS");
            let n: u32 = argv.get(3).map(|s| s.parse().unwrap()).unwrap_or(1_000_000);
            let calls: u64 = argv.get(4).map(|s| s.parse().unwrap()).unwrap_or(20_000_000);
            bench::<M>(def, n, calls);
        }
        _ => {
            eprintln!("usage: {} defs | run [SCRIPT] | bench DEF [ACCOUNTS] [CALLS]", M::NAME);
            std::process::exit(2);
        }
    }
}

fn find_def<M: Module>(name: &str) -> Option<usize> {
    M::DEFS.iter().position(|d| d.name == name)
}

/// Parse a world line's items into a fresh world. Accounts first, so map keys can name them.
pub fn parse_world<M: Module>(items: &[&str]) -> Result<World, String> {
    // items are tokens(): a Text entry `state.key:"…"` is one item
    let mut w = M::new_world();
    let mut later = Vec::new();
    for it in items {
        if let Some(n) = it.strip_prefix("now:") {
            w.clock = n.parse().map_err(|_| format!("bad now {it}"))?;
        } else if it.as_bytes()[0].is_ascii_uppercase() {
            let (name, rest) = it.split_once(':').ok_or(format!("bad item {it}"))?;
            let (b, o) = rest.split_once('@').unwrap_or((rest, "None"));
            w.open(name, b.parse().map_err(|_| format!("bad balance {it}"))?, o);
        } else {
            later.push(*it);
        }
    }
    for it in later {
        // `state.key:value`; the key has no `:` or `"`, a Text value may have both. A record's field state has dots
        // in its name (`order.qty.7:3`), a key never has one: the state is everything before the last dot
        let (k, v) = it.split_once(':').ok_or(format!("bad item {it}"))?;
        let (m, key) = k.rsplit_once('.').ok_or(format!("bad item {it}"))?;
        let mi = M::MAPS.iter().find(|x| x.name == m).ok_or(format!("no state {m}"))?;
        if mi.text {
            let t = unquote(v).ok_or(format!("bad text {it}"))?;
            match mi.key {
                KeyType::Account => {
                    let a = w.acct(key).ok_or(format!("no account {key}"))?;
                    w.atexts[mi.slot].insert(a.0, t);
                }
                _ => {
                    w.itexts[mi.slot].insert(key.parse().map_err(|_| format!("bad key {it}"))?, t);
                }
            }
            continue;
        }
        let v: i64 = v.parse().map_err(|_| format!("bad value {it}"))?;
        match mi.key {
            KeyType::Account => {
                let a = w.acct(key).ok_or(format!("no account {key}"))?;
                w.amaps[mi.slot][a.0 as usize] = v;
            }
            _ => {
                w.imaps[mi.slot].insert(key.parse().map_err(|_| format!("bad key {it}"))?, v);
            }
        }
    }
    Ok(w)
}

pub fn dump<M: Module>(w: &World) -> String {
    let mut s = format!("clock={} |", w.clock);
    for (i, n) in w.names.iter().enumerate() {
        s += &format!(" {n}:{}@{}", w.bal[i], w.owners[w.owner[i].0 as usize]);
    }
    s += " |";
    let mut maps: Vec<&MapInfo> = M::MAPS.iter().collect();
    maps.sort_by_key(|m| m.name);
    for m in maps {
        if m.text {
            // as the interpreter's dump: entries that aren't the default (""), sorted by key, quoted
            let mut ks: Vec<(String, i64, &String)> = match m.key {
                KeyType::Account => w.atexts[m.slot].iter().map(|(a, t)| (w.names.get(*a).to_string(), 0, t)).collect(),
                _ => w.itexts[m.slot].iter().map(|(k, t)| (String::new(), *k, t)).collect(),
            };
            ks.retain(|(_, _, t)| !t.is_empty());
            ks.sort();
            for (a, k, t) in ks {
                let key = if m.key == KeyType::Account { a } else { k.to_string() };
                s += &format!(" {}.{key}:{}", m.name, quote(t));
            }
            continue;
        }
        match m.key {
            KeyType::Account => {
                let mut ks: Vec<(String, i64)> = w.amaps[m.slot]
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| **v != 0)
                    .map(|(i, v)| (w.names.get(i as u32).to_string(), *v))
                    .collect();
                ks.sort();
                for (k, v) in ks {
                    s += &format!(" {}.{k}:{v}", m.name);
                }
            }
            _ => {
                let mut ks: Vec<(i64, i64)> = w.imaps[m.slot].iter().filter(|(_, v)| **v != 0).map(|(k, v)| (*k, *v)).collect();
                ks.sort();
                for (k, v) in ks {
                    s += &format!(" {}.{k}:{v}", m.name);
                }
            }
        }
    }
    s
}

/// Check and convert call arguments at the boundary, as run.py's `Interp.run` does.
pub fn parse_args<M: Module>(w: &World, def: usize, raw: &[&str]) -> Result<Vec<Val>, String> {
    // raw are tokens(): a Text argument is one quoted token
    let d = &M::DEFS[def];
    if raw.len() != d.params.len() {
        return Err(format!("{} takes {} argument(s), got {}", d.name, d.params.len(), raw.len()));
    }
    d.params
        .iter()
        .zip(raw)
        .map(|((p, t), s)| match t {
            KeyType::Account => w.acct(s).map(Val::Acct).ok_or(format!("account {s:?} for `{p}` is not in the world")),
            KeyType::Int => s.parse().map(Val::Int).map_err(|_| format!("`{p}` must be an Int, got {s:?}")),
            KeyType::Text => unquote(s).map(Val::Text).ok_or(format!("`{p}` must be a quoted Text, got {s}")),
        })
        .collect()
}

/// A value as script lines write it: an Int, an account by name, a Text quoted.
fn show_val(v: &Val, w: &World) -> String {
    match v {
        Val::Int(n) => n.to_string(),
        Val::Acct(a) => w.names.get(a.0).to_string(),
        Val::Text(t) => quote(t),
    }
}

pub fn run_script<M: Module>(input: Box<dyn BufRead>, out: &mut dyn Write) {
    let mut w = M::new_world();
    let mut exec: Exec<M::Event> = Exec::default();
    let mut prov = Replay::default();
    for line in input.lines() {
        let line = line.unwrap();
        let owned = tokens(&line);
        let toks: Vec<&str> = owned.iter().map(String::as_str).collect();
        match toks.first().copied() {
            None => {}
            Some("world") => match parse_world::<M>(&toks[1..]) {
                Ok(nw) => w = nw,
                Err(e) => panic!("bad world line {line:?}: {e}"),
            },
            Some("obs") => prov.0.extend(toks[1..].iter().map(|v| v.parse::<i64>().unwrap())),
            Some("dump") => writeln!(out, "{}", dump::<M>(&w)).unwrap(),
            Some("call") => {
                let line = match find_def::<M>(toks[1]).ok_or(()).and_then(|d| parse_args::<M>(&w, d, &toks[2..]).map(|a| (d, a)).map_err(|_| ())) {
                    Err(()) => format!("badcall t={}", w.clock),
                    Ok((d, args)) => {
                        let r = M::call(&mut exec, &mut w, &mut prov, d, &args);
                        prov.0.clear(); // each call gets exactly the values its `obs` line gave
                        let mut s = match r {
                            Ok(None) => "ok".to_string(),
                            // an Int, or a record's leaves in order as an `eg` line writes them (`-> ok 2 3`)
                            Ok(Some(vs)) => format!("ok result={}", vs.iter().map(|v| show_val(v, &w)).collect::<Vec<_>>().join(" ")),
                            Err(Stop::Refused(r)) => format!("refused {}", M::show_refusal(&r, &w)),
                            Err(Stop::Fault(_)) => "fault".to_string(),
                        };
                        s += &format!(" t={}", w.clock);
                        for (ci, c) in M::CHANNELS.iter().enumerate() {
                            let evs: Vec<String> = exec.events.iter().filter(|(ch, _)| *ch as usize == ci).map(|(_, e)| M::show_event(e, &w)).collect();
                            if !evs.is_empty() {
                                s += &format!(" | {c}: {}", evs.join(", "));
                            }
                        }
                        s
                    }
                };
                writeln!(out, "{line}").unwrap();
            }
            Some(other) => panic!("unknown script command {other:?}"),
        }
    }
}

#[repr(C)]
struct RUsage {
    t: [i64; 4],
    maxrss: i64,
    rest: [i64; 13],
}
extern "C" {
    fn getrusage(who: i32, r: *mut RUsage) -> i32;
}
fn maxrss() -> i64 {
    unsafe {
        let mut r: RUsage = std::mem::zeroed();
        getrusage(0, &mut r);
        r.maxrss
    }
}

/// One definition, `calls` times, over `n` accounts of 1000; arguments from the xorshift stream the
/// baseline used (k-th Account param: (x >> 20k) % n; Int params: (x >> 40) % 1500 - 10), so the
/// checksum for `transfer` is comparable with the hand-written baselines.
fn bench<M: Module>(def: &str, n: u32, calls: u64) {
    let d = find_def::<M>(def).expect("no such definition");
    let r0 = maxrss();
    let mut w = M::new_world();
    for i in 0..n {
        w.open(&format!("A{i}"), 1000, "u0");
    }
    let r1 = maxrss();
    let mut exec: Exec<M::Event> = Exec::default();
    let mut prov = Seeded(0x9E3779B97F4A7C15);
    let params = M::DEFS[d].params;
    let mut args = vec![Val::Int(0); params.len()];
    let (mut x, mut ok, mut refused, mut fault) = (88172645463325252u64, 0u64, 0u64, 0u64);
    let t0 = Instant::now();
    for _ in 0..calls {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let mut k = 0;
        for (i, (_, t)) in params.iter().enumerate() {
            args[i] = match t {
                KeyType::Account => {
                    let a = Val::Acct(Acct(((x >> (20 * k)) % n as u64) as u32));
                    k += 1;
                    a
                }
                KeyType::Int => Val::Int(((x >> 40) % 1500) as i64 - 10),
                KeyType::Text => Val::Text(String::new()),
            };
        }
        match M::call(&mut exec, &mut w, &mut prov, d, &args) {
            Ok(_) => ok += 1,
            Err(Stop::Refused(_)) => refused += 1,
            Err(Stop::Fault(_)) => fault += 1,
        }
        std::hint::black_box(&exec.events);
    }
    let dt = t0.elapsed().as_secs_f64();
    let cs: i64 = w.bal.iter().enumerate().map(|(i, b)| b * (i as i64 + 1)).sum();
    println!(
        "{} {def}: {:.1} ns/call ({:.1}M calls/s), {:.1} bytes/account, ok={ok} refused={refused} fault={fault} checksum={cs}",
        M::NAME,
        dt / calls as f64 * 1e9,
        calls as f64 / dt / 1e6,
        (r1 - r0) as f64 / n as f64
    );
}
