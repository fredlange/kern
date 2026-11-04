//! `kern`: the command-line front end (port of kern/__main__.py).
//!
//!   kern check  FILE.k [--why FILE.why] [--html OUT.html] [--quick] [--cache DIR] [--baseline F] [--against REV]
//!   kern view   FILE.k NAME --as iface|spec|editor [--why FILE.why]
//!   kern why    FILE.k NAME [--why FILE.why]
//!   kern anchor FILE.k [--why FILE.why]
//!   kern baseline FILE.k [--baseline FILE.strength]   record guarantee strength (the ratchet)
//!   kern run    FILE.k "transfer A B 100" --world "A:500@u1 B:0@u1"
//!   kern build  FILE.k [--out DIR]
//!
//! The arguments are parsed as Python's argparse parsed them (one positional command, a file, an optional name;
//! options anywhere, `--opt VALUE` or `--opt=VALUE`), so the commands and options GUIDE.md documents are unchanged.

use std::io::Write;

use indexmap::IndexMap;

use crate::package::{load, store_path};
use crate::pyrepr::repr_str;
use crate::run::{Interp, Outcome, Value, World};
use crate::syntax::{parse_world, split_maps, split_now, Key};
use crate::why;

pub const COMMANDS: [&str; 7] = ["check", "view", "why", "anchor", "run", "baseline", "build"];
pub const VIEWS: [&str; 3] = ["iface", "spec", "editor"];
/// option -> takes a value
pub const OPTIONS: [(&str, bool); 9] = [
    ("--why", true),
    ("--html", true),
    ("--as", true),
    ("--world", true),
    ("--baseline", true),
    ("--against", true),
    ("--quick", false),
    ("--cache", true),
    ("--out", true),
];

const USAGE: &str = "usage: kern [-h] [--why WHYFILE] [--html HTML] [--as {iface,spec,editor}] [--world WORLD]
            [--baseline BASEFILE] [--against REV] [--quick] [--cache CACHE] [--out OUT]
            {check,view,why,anchor,run,baseline,build} file [name]
       kern host ...      the host (SQLite state, HTTP, replay; docs/HOST.md)
       kern mcp           the MCP server on stdio";

#[derive(Debug, Default, Clone)]
pub struct Args {
    pub cmd: String,
    pub file: String,
    pub name: Option<String>,
    pub whyfile: Option<String>,
    pub html: Option<String>,
    pub view: String,
    pub world: Option<String>,
    pub basefile: Option<String>,
    pub rev: Option<String>,
    pub quick: bool,
    pub cache: String,
    pub out: Option<String>,
}

/// argparse-style parse; Err(message) is printed after the usage line, exit 2.
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut a = Args { view: "iface".into(), cache: ".kern-cache".into(), ..Default::default() };
    let mut pos: Vec<String> = vec![];
    let mut i = 0;
    let mut extra: Vec<String> = vec![];
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg == "--" {
            pos.extend(argv[i..].iter().cloned());
            break;
        }
        if arg.starts_with("--") && arg.len() > 2 {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            let Some((_, takes)) = OPTIONS.iter().find(|(o, _)| *o == flag) else {
                extra.push(arg.clone());
                continue;
            };
            let value = if *takes {
                match inline {
                    Some(v) => Some(v),
                    None if i < argv.len() && !(argv[i].starts_with("--") && argv[i].len() > 2) => {
                        i += 1;
                        Some(argv[i - 1].clone())
                    }
                    None => return Err(format!("argument {}: expected one argument", flag)),
                }
            } else {
                if inline.is_some() {
                    return Err(format!("argument {}: ignored explicit argument '{}'", flag, inline.unwrap()));
                }
                None
            };
            match flag.as_str() {
                "--why" => a.whyfile = value,
                "--html" => a.html = value,
                "--as" => {
                    let v = value.unwrap();
                    if !VIEWS.contains(&v.as_str()) {
                        return Err(format!(
                            "argument --as: invalid choice: {} (choose from 'iface', 'spec', 'editor')",
                            repr_str(&v)
                        ));
                    }
                    a.view = v;
                }
                "--world" => a.world = value,
                "--baseline" => a.basefile = value,
                "--against" => a.rev = value,
                "--quick" => a.quick = true,
                "--cache" => a.cache = value.unwrap(),
                "--out" => a.out = value,
                _ => unreachable!(),
            }
        } else {
            pos.push(arg.clone());
        }
    }
    if pos.is_empty() {
        return Err("the following arguments are required: cmd, file".into());
    }
    if !COMMANDS.contains(&pos[0].as_str()) {
        let choices: Vec<String> = COMMANDS.iter().map(|c| format!("'{}'", c)).collect();
        return Err(format!("argument cmd: invalid choice: {} (choose from {})", repr_str(&pos[0]), choices.join(", ")));
    }
    if pos.len() < 2 {
        return Err("the following arguments are required: file".into());
    }
    extra.extend(pos.iter().skip(3).cloned());
    if !extra.is_empty() {
        return Err(format!("unrecognized arguments: {}", extra.join(" ")));
    }
    a.cmd = pos[0].clone();
    a.file = pos[1].clone();
    a.name = pos.get(2).cloned();
    Ok(a)
}

/// Python repr of a dict-like sequence of pairs.
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

/// The line `kern run` prints: Python's print(outcome, after.bal, "audit:", after.audit[, "state:", after.maps]).
pub fn run_line(outcome: &Outcome, after: &World) -> String {
    let out = match outcome {
        Outcome::Ok => "('ok',)".to_string(),
        Outcome::Fail(c, a) => {
            let args: Vec<String> = a.iter().map(Value::repr).collect();
            format!("({}, [{}])", repr_str(c), args.join(", "))
        }
    };
    let bal = dict_repr(after.bal.iter(), |a| repr_str(a), |b| b.to_string());
    let audit: Vec<String> = after.audit.iter().map(Value::repr).collect();
    let mut line = format!("{} {} audit: [{}]", out, bal, audit.join(", "));
    if !after.maps.is_empty() {
        let maps = dict_repr(after.maps.iter(), |x| repr_str(x), |kv| dict_repr(kv.iter(), |k| key_repr(k), |v| v.to_string()));
        line.push_str(&format!(" state: {}", maps));
    }
    line
}

/// Run the CLI; returns the exit code. Output goes to `out` and `err`.
pub fn main_with(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match argv.first().map(|s| s.as_str()) {
        Some("-h") | Some("--help") => {
            let _ = writeln!(out, "{}", USAGE);
            return 0;
        }
        _ => {}
    }
    let a = match parse_args(argv) {
        Ok(a) => a,
        Err(m) => {
            let _ = writeln!(err, "{}\nkern: error: {}", USAGE, m);
            return 2;
        }
    };
    if a.cmd == "build" {
        let _ = writeln!(err, "error: `kern build` is not ported to the Rust toolchain yet (use `python -m kern build`)");
        return 1;
    }
    let defs = match load(&a.file, &IndexMap::new()) {
        Ok(d) => d,
        Err(e) => {
            let _ = writeln!(err, "{}: {}", a.file, e);
            return 2;
        }
    };
    let whyfile = a.whyfile.clone().unwrap_or_else(|| store_path(&a.file, ".why"));
    let store = match why::load(&whyfile) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "error: {}", e);
            return 2;
        }
    };
    let whys: IndexMap<String, why::Status> =
        defs.defs.iter().map(|(n, d)| (n.clone(), why::status(&store, d, &defs))).collect();
    let orphans = why::orphans(&store, &defs);

    match a.cmd.as_str() {
        "anchor" => {
            let left = match why::anchor(&whyfile, &defs) {
                Ok(l) => l,
                Err(e) => {
                    let _ = writeln!(err, "error: {}", e);
                    return 2;
                }
            };
            let _ = writeln!(out, "anchored {} to current builds", whyfile);
            for (n, entries) in left {
                let _ = writeln!(
                    err,
                    "warning: {} why entr{} for `{}`, which does not exist (renamed or deleted?); kept unchanged",
                    entries.len(),
                    if entries.len() == 1 { "y" } else { "ies" },
                    n
                );
            }
            0
        }
        "run" => {
            let Some(world) = &a.world else {
                let _ = writeln!(err, "error: `run` needs --world \"A:500@u1 ...\"");
                return 2;
            };
            let parsed = split_now(world)
                .and_then(|(rest, now)| split_maps(&rest).map(|(rest, maps)| (rest, now, maps)))
                .and_then(|(rest, now, maps)| parse_world(&rest, None).map(|w| (w, now, maps)));
            let (w, now, maps) = match parsed {
                Ok(x) => x,
                Err(e) => {
                    let _ = writeln!(err, "error: {}", e);
                    return 2;
                }
            };
            let mut world = World::from_example(&w, now, &maps);
            let call = a.name.clone().unwrap_or_default();
            let mut parts = call.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let args: Vec<Value> = parts
                .map(|x| {
                    let digits = x.trim_start_matches('-');
                    match x.parse::<crate::Int>() {
                        Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => Value::Int(n),
                        _ => Value::Str(x.to_string()),
                    }
                })
                .collect();
            let mut interp = Interp::new(&defs);
            match interp.run(&name, &args, &mut world) {
                Ok(outcome) => {
                    let _ = writeln!(out, "{}", run_line(&outcome, &world));
                    0
                }
                Err(e) => {
                    let _ = writeln!(err, "error: {}", e);
                    1
                }
            }
        }
        "why" => {
            let Some(name) = a.name.as_ref().filter(|n| defs.defs.contains_key(*n)) else {
                let n = a.name.clone().unwrap_or_else(|| "None".into());
                let note = if orphans.contains_key(&n) {
                    format!(" (but {} has orphaned entries for it)", whyfile)
                } else {
                    String::new()
                };
                let _ = writeln!(err, "{}: no definition named `{}`{}", a.file, n, note);
                return 2;
            };
            let w = &whys[name];
            if w.stale {
                let _ = writeln!(
                    out,
                    "(stale: anchored to {}, the interface is now {}; check the entries, then `kern anchor {}`)",
                    w.anchored_to.as_deref().unwrap_or(""),
                    w.current.as_deref().unwrap_or(""),
                    a.file
                );
            }
            let _ = writeln!(out, "{}", if w.entries.is_empty() { "(no why entries)".to_string() } else { w.entries.join("\n") });
            0
        }
        _ => {
            let _ = writeln!(
                err,
                "error: `kern {}` needs the prover and the report, which are not ported to Rust yet (use `python -m kern {}`)",
                a.cmd, a.cmd
            );
            1
        }
    }
}

pub fn main(argv: &[String]) -> i32 {
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    main_with(argv, &mut out, &mut err)
}
