//! The docs stay current (port of tests/test_docs.py): docs/REFERENCE.md and docs/GUIDE.md are checked against the
//! code.
//!
//! - every ```kern block parses and passes the checker; ```kern clean blocks also prove clean
//! - every handler, spec clause and declaration form is documented in REFERENCE.md
//! - every command and option of `kern` and `kern host` is documented in GUIDE.md
//! - every `kern::module::name` in GUIDE.md's API table exists in kern/src
//! - every MCP tool is documented in GUIDE.md
//! - REFERENCE.md stays within its token budget (docs/research/CHALLENGE.md, C2: 2,500)
//!
//! If one of these fails, the docs are behind the code: update them in the same commit.

use regex::Regex;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

fn read(p: &str) -> String {
    std::fs::read_to_string(format!("{ROOT}/{p}")).unwrap()
}

fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let rest = text.split_once(heading).unwrap_or_else(|| panic!("no section {heading:?}")).1;
    rest.split("\n## ").next().unwrap()
}

fn blocks() -> Vec<(String, String, String)> {
    let re = Regex::new(r"(?s)```(kern[^\n]*)\n(.*?)```").unwrap();
    let mut out = vec![];
    for f in ["docs/REFERENCE.md", "docs/GUIDE.md"] {
        let text = read(f);
        for c in re.captures_iter(&text) {
            out.push((f.to_string(), c[1].to_string(), c[2].to_string()));
        }
    }
    out
}

#[test]
fn every_kern_block_checks_and_clean_blocks_prove() {
    let bs = blocks();
    assert!(bs.len() >= 2);
    for (i, (f, info, body)) in bs.iter().enumerate() {
        let defs = kern::syntax::parse_module(body, true).unwrap_or_else(|e| panic!("{f} block {i}: {e}"));
        kern::wf::check_module(&defs).unwrap_or_else(|e| panic!("{f} block {i}: {}", e.message()));
        if info.split_whitespace().any(|w| w == "clean") {
            let reps = kern::report::analyse(&defs, false, None, None).unwrap();
            let dirty: Vec<&String> = reps.iter().filter(|(_, r)| !r.clean).map(|(n, _)| n).collect();
            assert!(dirty.is_empty(), "{f} block {i}: not clean: {dirty:?}");
        }
    }
}

#[test]
fn every_handler_and_language_form_is_documented() {
    let reference = read("docs/REFERENCE.md");
    for (name, _, _) in kern::run::HANDLERS {
        assert!(reference.contains(&format!("`{name}")), "handler {name} is not in REFERENCE.md");
    }
    for form in ["need", "ok otherwise", "keep", "ensure", "emit", "when", "need as", "emit as", "eg", "state",
                 "invariant", "channel", "-> Int", "result", "distinct", "if", "else"] {
        assert!(reference.contains(&format!("`{form}")), "form {form} is not in REFERENCE.md");
    }
}

fn documented(part: &str, cmds: &[&str], opts: &[(&str, bool)], what: &str) {
    for c in cmds {
        assert!(part.contains(&format!("| `{c} ")) || part.contains(&format!("| `{c}`")), "{what} command {c} is not in GUIDE.md");
    }
    for (o, _) in opts {
        assert!(part.contains(&format!("`{o}")), "{what} option {o} is not in GUIDE.md");
    }
}

#[test]
fn every_cli_command_and_option_is_documented() {
    let guide = read("docs/GUIDE.md");
    documented(section(&guide, "## `kern` commands"), &kern::cli::COMMANDS, &kern::cli::OPTIONS, "kern");
    documented(section(&guide, "## `kern host` commands"), &kern::host::COMMANDS, &kern::host::OPTIONS, "kern host");
}

#[test]
fn every_api_name_exists() {
    let guide = read("docs/GUIDE.md");
    let part = section(&guide, "## Rust library API");
    let names = Regex::new(r"`kern::([a-z_]+)::([A-Za-z_]+)").unwrap();
    let mut n = 0;
    for c in names.captures_iter(part) {
        let (module, name) = (&c[1], &c[2]);
        let src = std::fs::read_to_string(format!("{ROOT}/kern/src/{module}.rs"))
            .unwrap_or_else(|_| panic!("kern::{module} is in GUIDE.md but kern/src/{module}.rs doesn't exist"));
        let def = Regex::new(&format!(r"pub (fn|struct|enum|type|trait|const) {name}\b")).unwrap();
        assert!(def.is_match(&src), "kern::{module}::{name} is in GUIDE.md but not in the code");
        n += 1;
    }
    assert!(n >= 20, "only {n} API names found");
}

#[test]
fn every_mcp_tool_is_documented() {
    let guide = read("docs/GUIDE.md");
    let part = section(&guide, "## The MCP server");
    for t in kern::mcp::tools().as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        assert!(part.contains(&format!("`{name}`")), "MCP tool {name} is not in GUIDE.md");
    }
}

#[test]
fn reference_stays_within_its_token_budget() {
    match kern::views::tokens(&read("docs/REFERENCE.md")) {
        None => eprintln!("SKIP: no tokenizer"),
        Some(n) => assert!(n <= 2500, "REFERENCE.md is {n} tokens; the budget is 2,500 (docs/research/CHALLENGE.md, C2)"),
    }
}
