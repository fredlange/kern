//! Golden outputs: the regression floor for the parser, checker, prover, views and proof page once the Python
//! oracles are gone (AGENTS.md: output unchanged unless the change explains it).
//!
//! For every example (a package directory counts as one module) and every probe in tests/probes/,
//! tests/golden/<module>.txt holds `kern check`'s output and exit code (full check, mutation testing included)
//! and the iface, spec and editor views of every definition. A few examples also have their proof page in
//! tests/golden/<module>.html. Files are per module, so a diff shows what moved.
//!
//! A difference fails the test and names the files. If the change is intended, explain it in the commit and
//! re-record:
//!
//! ```text
//! KERN_BLESS=1 cargo test -p kern --release --test golden
//! ```
//!
//! The outputs were first recorded from the Rust toolchain on 2026-11-05, after its final comparison with the
//! Python toolchain (docs/research/RUST_PORT_PLAN.md).

mod common;

use indexmap::IndexMap;
use std::path::{Path, PathBuf};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// Proof pages recorded as HTML: (module, extra check arguments).
const PAGES: [(&str, &[&str]); 4] = [
    ("examples/ledger-tour/ledger_v3.k", &["--why", "examples/ledger-tour/ledger.why"]),
    ("examples/escrow/escrow_s2.k", &[]),
    ("examples/shop", &[]),
    ("examples/raffle/raffle.k", &[]),
];

/// Every example (a package directory as one module, otherwise each .k file) and every probe, relative to ROOT.
fn modules() -> Vec<String> {
    let mut out = vec![];
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(format!("{ROOT}/examples")).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    let ks = |d: &Path| {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |x| x == "k"))
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    for d in dirs {
        let rel = format!("examples/{}", d.file_name().unwrap().to_string_lossy());
        if d.join("kern.pkg").exists() {
            out.push(rel);
        } else {
            out.extend(ks(&d).into_iter().map(|k| format!("{rel}/{k}")));
        }
    }
    out.extend(ks(Path::new(&format!("{ROOT}/tests/probes"))).into_iter().map(|k| format!("tests/probes/{k}")));
    out
}

fn slug(module: &str) -> String {
    module.trim_end_matches(".k").replace('/', "__")
}

/// `kern check` as text: the command line, the exit code, stdout, and stderr if any. It runs the binary (a fresh
/// process, as a user runs it) with a fresh cache directory; the `cache:` line, which names that directory, is
/// dropped.
fn check(args: &[&str], cache: &Path) -> String {
    let cache_s = cache.to_string_lossy().into_owned();
    let mut full: Vec<&str> = args.to_vec();
    full.extend(["--cache", &cache_s]);
    let (code, out, err) = common::kern(&full, ROOT);
    let keep = |l: &&str| !l.starts_with("cache: ");
    let out: Vec<&str> = out.lines().filter(keep).collect();
    let err: Vec<&str> = err.lines().filter(keep).collect();
    let mut s = format!("$ kern {}   (exit {code})\n{}\n", args.join(" "), out.join("\n"));
    if !err.is_empty() {
        s.push_str(&format!("stderr:\n{}\n", err.join("\n")));
    }
    s
}

/// What `kern view MODULE NAME --as VIEW` prints for every definition and view, from one analysis of the module
/// (the CLI analyses once per call: 498 analyses instead of 30), built as cli.rs builds it. The token count the CLI
/// adds on stderr is left out: it exists only where a tokenizer is installed.
fn views(module: &str) -> String {
    use kern::{package, report, views, why};
    let path = format!("{ROOT}/{module}");
    let defs = package::load(&path, &IndexMap::new()).unwrap();
    let store = why::load(&package::store_path(&path, ".why")).unwrap();
    let whys: IndexMap<String, why::Status> = defs.defs.iter().map(|(n, d)| (n.clone(), why::status(&store, d, &defs))).collect();
    let reports = report::analyse(&defs, false, None, None).unwrap();
    let mut s = String::new();
    for name in defs.defs.keys() {
        for view in ["iface", "spec", "editor"] {
            let text = match view {
                "iface" => views::iface(&reports[name], &whys[name]),
                "spec" => views::spec(&reports[name], &whys[name]),
                _ => views::editor(&reports, name, &whys),
            };
            let lines: Vec<&str> = text.lines().collect();
            s.push_str(&format!("\n$ kern view {module} {name} --as {view}   (exit 0)\n{}\n", lines.join("\n")));
        }
    }
    s
}

fn record(module: &str) -> Vec<(String, String)> {
    let tmp = common::tmp(&format!("golden-{}", slug(module)));
    let txt = check(&["check", module], &tmp.join("cache")) + &views(module);
    let mut files = vec![(format!("{}.txt", slug(module)), txt)];
    if let Some((_, extra)) = PAGES.iter().find(|(m, _)| *m == module) {
        let html = tmp.join("page.html");
        let html_s = html.to_string_lossy().into_owned();
        let mut args = vec!["check", module, "--html", &html_s];
        args.extend(extra.iter());
        check(&args, &tmp.join("cache-html"));
        files.push((format!("{}.html", slug(module)), std::fs::read_to_string(&html).unwrap()));
    }
    let _ = std::fs::remove_dir_all(&tmp);
    files
}

#[test]
fn outputs_match_the_recorded_ones() {
    let mods = modules();
    assert!(mods.len() >= 20, "only {} modules", mods.len());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(vec![]);
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(m) = mods.get(i) else { break };
                let files = record(m);
                results.lock().unwrap().extend(files);
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort();
    let bless = std::env::var("KERN_BLESS").map_or(false, |v| v == "1");
    std::fs::create_dir_all(GOLDEN).unwrap();
    let mut bad = vec![];
    for (name, text) in &results {
        let path = format!("{GOLDEN}/{name}");
        if bless {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                std::fs::write(&path, text).unwrap();
                eprintln!("recorded {name}");
            }
            continue;
        }
        match std::fs::read_to_string(&path) {
            Err(_) => bad.push(format!("{name}: no recorded output")),
            Ok(want) if want != *text => {
                let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), text.lines().collect());
                let i = (0..w.len().max(g.len())).find(|&i| w.get(i) != g.get(i)).unwrap_or(0);
                bad.push(format!(
                    "{name}: differs from line {}\n  recorded: {}\n  now:      {}",
                    i + 1,
                    w.get(i).unwrap_or(&"<end>"),
                    g.get(i).unwrap_or(&"<end>")
                ));
            }
            Ok(_) => {}
        }
    }
    // a recorded file with no module behind it is stale: delete it (or re-record with KERN_BLESS=1, which does)
    let current: std::collections::BTreeSet<&String> = results.iter().map(|(n, _)| n).collect();
    for e in std::fs::read_dir(GOLDEN).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !current.contains(&n) {
            if bless {
                std::fs::remove_file(e.path()).unwrap();
                eprintln!("removed {n}");
            } else {
                bad.push(format!("{n}: recorded, but no module produces it any more"));
            }
        }
    }
    eprintln!("{} files from {} modules, {} differences", results.len(), mods.len(), bad.len());
    assert!(
        bad.is_empty(),
        "golden outputs changed (if intended, explain it in the commit and re-record with \
         KERN_BLESS=1 cargo test -p kern --release --test golden):\n{}",
        bad.join("\n")
    );
}
