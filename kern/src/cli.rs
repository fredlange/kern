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
    run_line_with(outcome, after, &[])
}

/// run_line, with the Decimal states' entries shown with their point (`12.5`, not the scaled 125000).
pub fn run_line_with(outcome: &Outcome, after: &World, decimal_states: &[String]) -> String {
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
        let parts: Vec<String> = after
            .maps
            .iter()
            .map(|(x, kv)| {
                let dec = decimal_states.iter().any(|d| d == x);
                let v = |v: &crate::Int| if dec { crate::syntax::decimal_text(*v) } else { v.to_string() };
                format!("{}: {}", repr_str(x), dict_repr(kv.iter(), |k| key_repr(k), |n| v(n)))
            })
            .collect();
        let maps = format!("{{{}}}", parts.join(", "));
        line.push_str(&format!(" state: {}", maps));
    }
    if !after.texts.is_empty() {
        let texts = dict_repr(after.texts.iter(), |x| repr_str(x), |kv| dict_repr(kv.iter(), |k| key_repr(k), |v| repr_str(v)));
        line.push_str(&format!(" text: {}", texts));
    }
    if !after.lists.is_empty() {
        let list = |xs: &&Vec<crate::Int>| format!("[{}]", xs.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "));
        let lists = dict_repr(after.lists.iter(), |x| repr_str(x), |kv| dict_repr(kv.iter(), |k| key_repr(k), list));
        line.push_str(&format!(" list: {}", lists));
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
        return match crate::emit_rust::emit(&a.file, a.out.as_deref()).and_then(|d| crate::emit_rust::build(&d)) {
            Ok(bin) => {
                let _ = writeln!(out, "{}", bin);
                0
            }
            Err(e) => {
                let _ = writeln!(err, "error: {}", e);
                1
            }
        };
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
            let call = a.name.clone().unwrap_or_default();
            // Text in the call or the world: parse both as an `eg` line does (quote-aware)
            let (mut world, name, args): (World, String, Vec<Value>) = if call.contains('"') || world.contains('"') || call.contains('[') || world.contains('[') || !defs.account_states.is_empty() || !defs.decimal_states.is_empty() {
                match crate::syntax::parse_call(world, &call).and_then(|mut ex| {
                    crate::syntax::normalize_example(&defs.decimal_states, &mut ex)?;
                    Ok(ex)
                }) {
                    Ok(ex) => (
                        World::of_example(&ex),
                        ex.call.0.clone(),
                        crate::run::example_args(&ex),
                    ),
                    Err(e) => {
                        let _ = writeln!(err, "error: {}", e);
                        return 2;
                    }
                }
            } else {
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
                let mut parts = call.split_whitespace();
                let name = parts.next().unwrap_or("").to_string();
                let args: Vec<Value> = parts
                    .map(|x| {
                        let digits = x.trim_start_matches('-');
                        match x.parse::<crate::Int>() {
                            Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => Value::Int(n),
                            _ => crate::syntax::decimal_value(x).map(crate::run::decimal).unwrap_or_else(|_| Value::Str(x.to_string())),
                        }
                    })
                    .collect();
                (World::from_example(&w, now, &maps), name, args)
            };
            let mut interp = Interp::new(&defs);
            match interp.run(&name, &args, &mut world) {
                Ok(outcome) => {
                    let mut line = run_line_with(&outcome, &world, &defs.decimal_states);
                    // a definition with `-> Int` returned this: ('ok', 7)
                    if let (Outcome::Ok, true, Some(v)) = (&outcome, defs.defs[&name].ret.is_some(), &interp.last_result) {
                        let shown = match v {
                            Value::Ctor(c, xs) if c == crate::run::DEC => crate::syntax::decimal_text(xs[0].num().unwrap_or(0)),
                            v => v.repr(),
                        };
                        line = line.replacen("('ok',)", &format!("('ok', {})", shown), 1);
                    }
                    let _ = writeln!(out, "{}", line);
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
        _ => check_view_baseline(&a, &defs, &whys, &orphans, out, err),
    }
}

/// `check`, `view` and `baseline`: the commands that need the prover's report.
fn check_view_baseline(a: &Args, defs: &crate::syntax::Module, whys: &IndexMap<String, why::Status>,
                       orphans: &IndexMap<String, Vec<String>>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    use crate::{baseline as basemod, render, report::analyse, views};
    let basefile = a.basefile.clone().unwrap_or_else(|| store_path(&a.file, ".strength"));
    if a.cmd == "baseline" {
        let reports = match analyse(defs, true, None, None) {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(err, "{}: {}", a.file, e.message());
                return 2;
            }
        };
        if let Err(e) = basemod::write(&basefile, &reports, defs) {
            let _ = writeln!(err, "error: {}: {}", basefile, e);
            return 2;
        }
        let _ = writeln!(out, "recorded guarantee strength in {}", basefile);
        return 0;
    }
    let mut base = None;
    if a.cmd == "check" {
        if let Some(rev) = &a.rev {
            // a committed baseline can't be rewritten by the edit
            let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let rel = crate::package::relpath(&basefile, &cwd);
            let p = std::process::Command::new("git").args(["show", &format!("{}:./{}", rev, rel)]).output();
            match p {
                Ok(p) if p.status.success() => base = Some(basemod::parse(&String::from_utf8_lossy(&p.stdout))),
                Ok(p) => {
                    let msg: String = String::from_utf8_lossy(&p.stderr).trim().chars().take(200).collect();
                    let _ = writeln!(err, "{}: no baseline {} at {}: {}", a.file, rel, rev, msg);
                    return 2;
                }
                Err(e) => {
                    let _ = writeln!(err, "{}: no baseline {} at {}: {}", a.file, rel, rev, e);
                    return 2;
                }
            }
        } else {
            base = Some(basemod::load(&basefile));
        }
    }
    let check = a.cmd == "check";
    let cache = if check && !a.cache.is_empty() { Some(a.cache.as_str()) } else { None };
    let base = base.filter(|b| !b.is_empty());
    let reports = match analyse(defs, check && !a.quick, base.as_ref(), cache) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "{}: {}", a.file, e.message());
            return 2;
        }
    };
    if let Some(c) = cache {
        let n = reports.values().filter(|r| r.cached).count();
        let _ = writeln!(err, "cache: {}/{} definitions reused ({})", n, reports.len(), c);
    }
    if a.cmd == "view" {
        let name = a.name.clone().unwrap_or_default();
        if !reports.contains_key(&name) {
            let _ = writeln!(err, "{}: no definition named `{}`", a.file, name);
            return 2;
        }
        let text = match a.view.as_str() {
            "iface" => views::iface(&reports[&name], &whys[&name]),
            "spec" => views::spec(&reports[&name], &whys[&name]),
            _ => views::editor(&reports, &name, whys),
        };
        let _ = writeln!(out, "{}", text);
        if let Some(n) = views::tokens(&text) {
            let _ = writeln!(err, "\n[{} tokens]", n);
        }
        return 0;
    }
    let _ = writeln!(out, "{}", render::terminal(&reports, defs, whys, orphans));
    if let Some(html) = &a.html {
        let title = a.file.rsplit('/').next().unwrap_or(&a.file);
        if let Some(dir) = std::path::Path::new(html).parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir); // a missing build/ is created; a real failure shows in the write
        }
        if let Err(e) = std::fs::write(html, render::page(&reports, defs, whys, title, orphans, None)) {
            let _ = writeln!(err, "error: {}: {}", html, e);
            return 2;
        }
        let _ = writeln!(out, "proof page: {}", html);
    }
    if reports.values().all(|r| r.clean) {
        0
    } else {
        1
    }
}

pub fn main(argv: &[String]) -> i32 {
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    main_with(argv, &mut out, &mut err)
}

