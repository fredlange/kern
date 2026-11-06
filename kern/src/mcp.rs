//! An MCP server for Kern (port of kern/mcp.py): an agent works through views, never through whole files.
//!
//! ```text
//! kern mcp            (stdio, JSON-RPC 2.0, one message per line)
//! ```
//!
//! Tools (see docs/GUIDE.md): kern_list, kern_view, kern_why, kern_check, kern_run, kern_callers, kern_edit,
//! kern_add, kern_replace, kern_remove.
//!
//! Edits are structural. The whole new module is parsed and checked for well-formedness first; if it is
//! ill-formed, the file is unchanged. Otherwise the file is written, and the edited definition and every transitive
//! caller are re-checked (callers were verified against the old spec). The result is ok only if all of them are
//! clean; otherwise it says what broke, as `check` does for a file.
//!
//! Responses are the Python server's, byte for byte (pyjson), apart from the session stubs below. One deliberate
//! difference: a runtime error in
//! `kern_run` (Python's KernError) is a tool error here; the Python server let it escape and stopped.
//!
//! One session (a `kern mcp` process, `Session`) remembers the latest thing it showed about each definition. A
//! later editor view sends a callee's spec block as a one-line stub when that latest showing still holds: the same
//! block, word for word (as a callee block or a spec view: `NAME: spec shown earlier in this session`), or an
//! editor view of NAME whose lines before `body`, without `eg` lines, are the block without its proof line (the
//! stub then carries the current proof line). A block with lifted `body need` lines or expanded `need as` lines
//! never equals an editor view, so it is sent. `full: true` sends every
//! block (an agent whose context was compacted). The test is on text, not on the definition's hash: a callee's
//! block also changes when a callee *of it* changes (its inferred `fails`, its proof line), which its own hash
//! doesn't see. An edit therefore never leaves a stale stub. C2 measured the repeats
//! (experiments/challenge/c2/PROTOCOL.md, 2026-11-05).
//! `kern view` and the free `handle`/`call_tool` are stateless: they never stub.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Write};

use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::infer::callees;
use crate::package::{self, abspath, read_text};
use crate::pyjson;
use crate::pyrepr::repr_str;
use crate::render;
use crate::report::{analyse, Report};
use crate::run::{Interp, Outcome, Value as V, World};
use crate::syntax::{parse_module, parse_world, split_maps, split_now, Key, Module};
use crate::views;
use crate::why;

pub const PROTOCOL: &str = "2024-11-05";

pub fn tools() -> Value {
    let file = json!({"type": "string", "description": "path to the .k module"});
    let name = json!({"type": "string", "description": "definition name"});
    json!([
        {"name": "kern_list", "description": "List the definitions of a module: interface line and status.",
         "inputSchema": {"type": "object", "properties": {"file": file}, "required": ["file"]}},
        {"name": "kern_view", "description": "A view of one definition. To read one, use spec (interface + guarantees). Use editor (full source + callees' specs) only for a definition you will change. iface: the interface only. Callee specs already sent come back as a one-line stub; full: true resends them.",
         "inputSchema": {"type": "object", "properties": {"file": file, "name": name,
                                                          "as": {"type": "string", "enum": ["iface", "spec", "editor"]},
                                                          "full": {"type": "boolean"}},
                         "required": ["file", "name"]}},
        {"name": "kern_why", "description": "Rationale (why) entries for a definition, with staleness.",
         "inputSchema": {"type": "object", "properties": {"file": file, "name": name}, "required": ["file", "name"]}},
        {"name": "kern_check", "description": "Check a module. quick (default true) skips mutation testing. Without name: a summary (status per definition, plus what is broken); with name: full detail.",
         "inputSchema": {"type": "object", "properties": {"file": file, "quick": {"type": "boolean"}, "name": name},
                         "required": ["file"]}},
        {"name": "kern_run", "description": "Run one call in the interpreter, e.g. call 'transfer A B 100', world 'A:500@u1 B:0@u2'.",
         "inputSchema": {"type": "object", "properties": {"file": file, "call": {"type": "string"}, "world": {"type": "string"}},
                         "required": ["file", "call", "world"]}},
        {"name": "kern_callers", "description": "Definitions that call (reach) this one, transitively: whose guarantees a change can break.",
         "inputSchema": {"type": "object", "properties": {"file": file, "name": name}, "required": ["file", "name"]}},
        {"name": "kern_edit", "description": "Apply several edits together, then check once: ops = [{op: add, source} | {op: replace, name, source, expect_hash?} | {op: remove, name, expect_hash?}]. If the result is ill-formed, nothing is written.",
         "inputSchema": {"type": "object", "properties": {"file": file, "ops": {"type": "array", "items": {"type": "object"}}},
                         "required": ["file", "ops"]}},
        {"name": "kern_add", "description": "Add one definition (its full source: header, spec, body), or one declaration line (const, record, state, channel, invariant).",
         "inputSchema": {"type": "object", "properties": {"file": file, "source": {"type": "string"}}, "required": ["file", "source"]}},
        {"name": "kern_replace", "description": "Replace one definition with new source (same name). expect_hash: refuse if its build hash is no longer this.",
         "inputSchema": {"type": "object", "properties": {"file": file, "name": name, "source": {"type": "string"},
                                                          "expect_hash": {"type": "string"}},
                         "required": ["file", "name", "source"]}},
        {"name": "kern_remove", "description": "Remove one definition; refused while anything references it.",
         "inputSchema": {"type": "object", "properties": {"file": file, "name": name, "expect_hash": {"type": "string"}},
                         "required": ["file", "name"]}},
    ])
}

/// A tool's failure: (Python exception type name, message), shown as "Type: message".
#[derive(Debug)]
pub struct ToolError(pub &'static str, pub String);

type T<X> = Result<X, ToolError>;

fn tool<X>(msg: impl Into<String>) -> T<X> {
    Err(ToolError("ToolError", msg.into()))
}

// --- the module as text: definitions are column-0 blocks ------------------------------------

fn is_col0(l: &str) -> bool {
    l.chars().next().map_or(false, |c| !c.is_whitespace())
}

/// (lines, start, end) of definition `name`'s block (header to the line before the next column-0 line that isn't
/// `body`), trailing blank lines excluded.
fn span(text: &str, name: &str) -> T<(Vec<String>, usize, usize)> {
    let lines: Vec<String> = text.split('\n').map(String::from).collect();
    let re = Regex::new(&format!(r"^{}(\s|$)", regex::escape(name))).unwrap();
    let Some(start) = lines.iter().position(|l| re.is_match(l)) else {
        return tool(format!("no definition named {}", repr_str(name)));
    };
    let mut end = start + 1;
    while end < lines.len() && !(is_col0(&lines[end]) && lines[end].trim() != "body") {
        end += 1;
    }
    while end > start + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    Ok((lines, start, end))
}

fn strip_nl(s: &str) -> &str {
    s.trim_matches('\n')
}

/// The name of the single definition in `source`. Syntax errors are reported with line numbers relative to the
/// submitted definition; references to other definitions (`need as F`) are resolved later, against the module.
fn one_def(source: &str) -> T<String> {
    let heads: Vec<&str> = strip_nl(source).split('\n').filter(|l| is_col0(l) && l.trim() != "body").collect();
    if heads.len() != 1 {
        return tool("the source must be exactly one definition (one header line at column 0)");
    }
    let m = Regex::new(r"^([a-z]\w*)").unwrap().captures(heads[0]).map(|c| c[1].to_string());
    let h = heads[0];
    let Some(name) = m.filter(|_| !["state ", "invariant ", "channel "].iter().any(|k| h.starts_with(k))
        && !(h.starts_with("const ") && !h.contains(':'))
        && !(h.starts_with("record ") && h["record ".len()..].starts_with(|c: char| c.is_uppercase())))
    else {
        return tool(format!("not a definition header: {}", repr_str(h)));
    };
    if let Err(e) = parse_module(source, true) {
        if !e.0.contains("`as ") {
            return tool(format!("does not parse (lines count from the submitted definition): {}", e));
        }
    }
    Ok(name)
}

/// One column-0 declaration line: `const`, `record`, `state`, `channel` or `invariant` (not a definition named like
/// one: `const x:Int` and `record x:Int` are headers).
fn is_declaration(source: &str) -> bool {
    let src = strip_nl(source);
    if src.contains('\n') || !is_col0(src) {
        return false;
    }
    ["state ", "invariant ", "channel "].iter().any(|k| src.starts_with(k))
        || (src.starts_with("const ") && !src.contains(':'))
        || (src.starts_with("record ") && src["record ".len()..].starts_with(|c: char| c.is_uppercase()))
}

fn realpath(p: &str) -> String {
    std::fs::canonicalize(p).map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|_| abspath(p))
}

/// KERN_MCP_ROOT=dir: the server only touches modules under dir (C2's isolation).
fn inside_root(path: &str) -> T<()> {
    if let Ok(root) = std::env::var("KERN_MCP_ROOT") {
        if !root.is_empty() && !realpath(path).starts_with(&format!("{}/", realpath(&root))) {
            return tool(format!("{} is outside this server's root", path));
        }
    }
    Ok(())
}

fn os_error(e: std::io::Error, path: &str) -> ToolError {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            ToolError("FileNotFoundError", format!("[Errno 2] No such file or directory: {}", repr_str(path)))
        }
        std::io::ErrorKind::PermissionDenied => {
            ToolError("PermissionError", format!("[Errno 13] Permission denied: {}", repr_str(path)))
        }
        _ => ToolError("OSError", format!("{}: {}", path, e)),
    }
}

fn load(path: &str) -> T<(String, Module)> {
    inside_root(path)?;
    let text = read_text(path).map_err(|e| os_error(e, path))?;
    match package::load(path, &IndexMap::new()) {
        Ok(defs) => {
            for f in package::files_of(&defs).values() {
                inside_root(f)?;
            }
            Ok((text, defs))
        }
        Err(e) => tool(format!("{}: {}", path, e)),
    }
}

fn whys(path: &str, defs: &Module) -> T<IndexMap<String, why::Status>> {
    let store = why::load(&package::store_path(path, ".why")).map_err(|e| ToolError("WhyFileError", e.0))?;
    Ok(defs.defs.iter().map(|(n, d)| (n.clone(), why::status(&store, d, defs))).collect())
}

fn cache(path: &str) -> String {
    let a = abspath(path);
    format!("{}/.kern-cache", &a[..a.rfind('/').unwrap_or(0)])
}

fn status(r: &Report) -> String {
    render::status(r).1
}

fn reports(defs: &Module, mutation: bool, path: &str) -> T<IndexMap<String, Report>> {
    analyse(defs, mutation, None, Some(&cache(path))).map_err(|e| ToolError("ToolError", e.message()))
}

/// Every definition that reaches `name`, transitively.
fn callers(defs: &Module, name: &str) -> BTreeSet<String> {
    let (mut out, mut frontier) = (BTreeSet::new(), BTreeSet::from([name.to_string()]));
    while !frontier.is_empty() {
        let new: BTreeSet<String> = defs
            .defs
            .iter()
            .filter(|(n, d)| !out.contains(*n) && callees(defs, d).iter().any(|c| frontier.contains(c)))
            .map(|(n, _)| n.clone())
            .collect();
        out.extend(new.iter().cloned());
        frontier = new;
    }
    out
}

fn op_str<'a>(op: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    op.get(k).and_then(|v| v.as_str())
}

/// One structural edit on the module text: (new text, touched definition).
fn apply(text: &str, defs: &Module, op: &Map<String, Value>, file: &str) -> T<(String, Option<String>)> {
    let kind = op_str(op, "op");
    let name = op_str(op, "name").unwrap_or("");
    if matches!(kind, Some("replace") | Some("remove")) {
        let Some(d) = defs.defs.get(name) else { return tool(format!("no definition named {}", repr_str(name))) };
        if let Some(home) = &d.file {
            if abspath(home) != abspath(file) {
                return tool(format!("{} is defined in {}; pass that file", name, home));
            }
        }
        if let Some(h) = op_str(op, "expect_hash").filter(|h| !h.is_empty()) {
            if d.hash() != h {
                return tool(format!("{} changed underneath: its build hash is {}, not {}", name, d.hash(), h));
            }
        }
    }
    let source = op_str(op, "source").unwrap_or("");
    match kind {
        Some("add") if is_declaration(source) => {
            // a module declaration: one column-0 line, appended; the whole module is validated before writing
            Ok((format!("{}\n\n{}\n", text.trim_end_matches('\n'), strip_nl(source)), None))
        }
        Some("add") => {
            let new = one_def(source)?;
            if defs.defs.contains_key(&new) {
                return tool(format!("{} already exists; use replace", new));
            }
            Ok((format!("{}\n\n{}\n", text.trim_end_matches('\n'), strip_nl(source)), Some(new)))
        }
        Some("replace") => {
            let got = one_def(source)?;
            if got != name {
                return tool(format!(
                    "the source defines {}, not {} (renaming isn't an edit here)",
                    repr_str(&got),
                    repr_str(name)
                ));
            }
            let (lines, start, end) = span(text, name)?;
            let mut out: Vec<String> = lines[..start].to_vec();
            out.extend(strip_nl(source).split('\n').map(String::from));
            out.extend(lines[end..].iter().cloned());
            Ok((out.join("\n"), Some(name.to_string())))
        }
        Some("remove") => {
            let users: Vec<String> = callers(defs, name).into_iter().filter(|n| n != name).collect();
            let refs: Vec<String> = defs
                .defs
                .iter()
                .filter(|(n, d)| *n != name && d.ref_origin.values().any(|c| c == name))
                .map(|(n, _)| n.clone())
                .collect();
            if !users.is_empty() || !refs.is_empty() {
                let all: BTreeSet<String> = users.into_iter().chain(refs).collect();
                return tool(format!("refused: {} reference {}", all.into_iter().collect::<Vec<_>>().join(", "), name));
            }
            let (lines, start, mut end) = span(text, name)?;
            while end < lines.len() && lines[end].trim().is_empty() {
                end += 1;
            }
            let mut out: Vec<String> = lines[..start].to_vec();
            out.extend(lines[end..].iter().cloned());
            Ok((format!("{}\n", out.join("\n").trim_end_matches('\n')), None))
        }
        other => tool(format!(
            "unknown op {} (add, replace, remove)",
            other.map(repr_str).unwrap_or_else(|| "None".into())
        )),
    }
}

/// Apply ops in order, validate the whole result once; ill-formed -> nothing written. Then write, re-check the
/// touched definitions and every transitive caller, and report what broke.
pub fn kern_edit(file: &str, ops: &[Value]) -> T<Value> {
    let (mut text, mut defs) = load(file)?;
    let old_whys = whys(file, &defs)?;
    let mut touched = vec![];
    for op in ops {
        let Some(op) = op.as_object() else { return Err(ToolError("AttributeError", "'list' object has no attribute 'get'".into())) };
        let (t, name) = apply(&text, &defs, op, file)?;
        text = t;
        // later ops see earlier ones; full check below
        let parsed = if package::is_package_file(file, Some(&text)) {
            package::load(file, &IndexMap::from([(file.to_string(), text.clone())]))
        } else {
            parse_module(&text, true)
        };
        defs = parsed.map_err(|e| ToolError("ToolError", format!("rejected, file unchanged: {}", e)))?;
        touched.extend(name);
    }
    if let Err(e) = crate::wf::check_module(&defs) {
        return tool(format!("rejected, file unchanged: {}", e.message()));
    }
    std::fs::write(file, &text).map_err(|e| os_error(e, file))?;
    let mut affected: BTreeSet<String> = touched.iter().filter(|t| defs.defs.contains_key(*t)).cloned().collect();
    for t in &touched {
        affected.extend(callers(&defs, t));
    }
    let reps = reports(&defs, false, file)?;
    let w = whys(file, &defs)?;
    let report: Map<String, Value> = affected.iter().map(|n| (n.clone(), json!(status(&reps[n])))).collect();
    let broken: Vec<String> =
        report.iter().filter(|(_, s)| s.as_str() != Some("All guarantees proven")).map(|(n, _)| n.clone()).collect();
    let store = why::load(&package::store_path(file, ".why")).map_err(|e| ToolError("WhyFileError", e.0))?;
    let details = if broken.is_empty() {
        String::new()
    } else {
        let sub: IndexMap<String, Report> = broken.iter().map(|n| (n.clone(), reps[n].clone())).collect();
        render::terminal(&sub, &defs, &w, &IndexMap::new())
    };
    let mut rechecked: Vec<&String> = reps.iter().filter(|(_, r)| !r.cached).map(|(n, _)| n).collect();
    rechecked.sort();
    let mut now_stale: Vec<&String> =
        w.iter().filter(|(n, s)| s.stale && !old_whys.get(*n).map_or(false, |o| o.stale)).map(|(n, _)| n).collect();
    now_stale.sort();
    let mut orphaned: Vec<String> = why::orphans(&store, &defs).into_keys().collect();
    orphaned.sort();
    // packages that `use` this one: their callers of what changed are re-checked too (F2: an agent skips a
    // body because its callers are checked against the new spec)
    let mut users = Map::new();
    let mut broken = broken;
    let within = std::env::var("KERN_MCP_ROOT").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| ".".to_string());
    for user in package::users_of(file, &within) {
        let udefs = package::load(&user, &IndexMap::new())
            .map_err(|e| ToolError("ToolError", format!("{} (a package that uses this one): {}", user, e.0)))?;
        let mut hit: BTreeSet<String> = BTreeSet::new();
        for t in &touched {
            hit.extend(callers(&udefs, t).into_iter().filter(|n| udefs.defs[n.as_str()].file.as_deref().map(package::root_of) != Some(package::root_of(file))));
        }
        if hit.is_empty() {
            continue;
        }
        let ureps = reports(&udefs, false, &user)?;
        let st: Map<String, Value> = hit.iter().map(|n| (n.clone(), json!(status(&ureps[n])))).collect();
        broken.extend(st.iter().filter(|(_, s)| s.as_str() != Some("All guarantees proven")).map(|(n, _)| format!("{user}: {n}")));
        users.insert(user.clone(), Value::Object(st));
    }
    Ok(json!({"ok": broken.is_empty(), "affected": report, "broken": broken, "rechecked": rechecked,
              "details": details, "why_now_stale": now_stale, "why_orphaned": orphaned, "users": users,
              "users_searched": package::abspath(&within)}))
}

// --- tools -----------------------------------------------------------------------------------

fn need_def(defs: &Module, name: &str) -> T<()> {
    if defs.defs.contains_key(name) {
        Ok(())
    } else {
        tool(format!("no definition named {}", repr_str(name)))
    }
}

pub fn kern_list(file: &str) -> T<Value> {
    let (_, defs) = load(file)?;
    let reps = reports(&defs, false, file)?;
    let w = whys(file, &defs)?;
    let list: Vec<Value> = reps
        .iter()
        .map(|(n, r)| {
            json!({"name": n, "iface": views::iface(r, &w[n]).split('\n').next().unwrap_or(""), "status": status(r)})
        })
        .collect();
    Ok(json!({ "definitions": list }))
}

/// The latest thing one session showed about a definition: its spec block, or an editor view of it (kept as the
/// spec part of its own text, `editor_spec`).
enum Shown {
    Block(String),
    Own(String),
}

/// The part of an editor view's own text that a spec block repeats: the lines before `body`, without `eg` lines.
fn editor_spec(own: &str) -> String {
    own.split('\n').take_while(|l| *l != "body").filter(|l| !l.starts_with("  eg ")).collect::<Vec<_>>().join("\n")
}

/// What one MCP session has shown, per (module path, definition name): only the latest, so a stub always means
/// "what you saw last is still true".
#[derive(Default)]
pub struct Session {
    shown: HashMap<(String, String), Shown>,
}

impl Session {
    /// The editor view `text` of `name`, with every callee block whose latest showing in this session still holds
    /// replaced by a stub (none with `full`). If the text doesn't end in the callee blocks as expected (views.rs
    /// changed its layout), it is sent whole and nothing is remembered.
    fn editor(&mut self, file: &str, reps: &IndexMap<String, Report>, name: &str, w: &IndexMap<String, why::Status>,
              text: String, full: bool) -> String {
        let blocks: Vec<(&String, String)> = reps[name].callees.iter().map(|c| (c, views::spec(&reps[c], &w[c]))).collect();
        let tail: String = blocks.iter().map(|(_, b)| format!("\n\n{}", b)).collect();
        if !text.ends_with(&tail) {
            return text;
        }
        let own = &text[..text.len() - tail.len()];
        let path = realpath(file);
        let mut out = own.to_string();
        for (c, b) in blocks {
            out.push_str("\n\n");
            let key = (path.clone(), c.clone());
            let (rest, proof) = b.rsplit_once('\n').unwrap_or(("", &b));
            match self.shown.get(&key) {
                Some(Shown::Block(t)) if !full && *t == b => {
                    out.push_str(&format!("{}: spec shown earlier in this session", c));
                }
                Some(Shown::Own(seen)) if !full && proof.starts_with("  proof ") && seen == rest => {
                    out.push_str(&format!("{}: spec shown earlier in this session; {}", c, proof.trim()));
                }
                _ => {
                    out.push_str(&b);
                    self.shown.insert(key, Shown::Block(b));
                }
            }
        }
        self.shown.insert((path, name.to_string()), Shown::Own(editor_spec(own)));
        out
    }

    /// One JSON-RPC message -> the response, or None for a notification.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        handle_in(self, msg)
    }

    pub fn call_tool(&mut self, name: &str, args: &Map<String, Value>) -> Option<T<Value>> {
        call_tool_in(self, name, args)
    }
}

/// A view as `kern view` prints it (no session: nothing is stubbed).
pub fn kern_view(file: &str, name: &str, as_: &str) -> T<Value> {
    kern_view_in(&mut Session::default(), file, name, as_, true)
}

fn kern_view_in(session: &mut Session, file: &str, name: &str, as_: &str, full: bool) -> T<Value> {
    let (_, defs) = load(file)?;
    need_def(&defs, name)?;
    let reps = reports(&defs, false, file)?;
    let w = whys(file, &defs)?;
    let text = match as_ {
        "iface" => views::iface(&reps[name], &w[name]),
        "spec" => {
            let t = views::spec(&reps[name], &w[name]);
            session.shown.insert((realpath(file), name.to_string()), Shown::Block(t.clone()));
            t
        }
        "editor" => session.editor(file, &reps, name, &w, views::editor(&reps, name, &w), full),
        other => return Err(ToolError("KeyError", repr_str(other))),
    };
    Ok(json!({"text": text, "tokens": views::tokens(&text)}))
}

pub fn kern_why(file: &str, name: &str) -> T<Value> {
    let (_, defs) = load(file)?;
    need_def(&defs, name)?;
    let w = &whys(file, &defs)?[name];
    Ok(json!({"entries": w.entries, "stale": w.stale}))
}

pub fn kern_check(file: &str, quick: bool, name: Option<&str>) -> T<Value> {
    let (_, defs) = load(file)?;
    let reps = reports(&defs, !quick, file)?;
    let w = whys(file, &defs)?;
    if let Some(name) = name {
        let Some(r) = reps.get(name) else { return tool(format!("no definition named {}", repr_str(name))) };
        let one = IndexMap::from([(name.to_string(), r.clone())]);
        return Ok(json!({"text": render::terminal(&one, &defs, &w, &IndexMap::new()), "status": status(r)}));
    }
    let bad: IndexMap<String, Report> = reps.iter().filter(|(_, r)| !r.clean).map(|(n, r)| (n.clone(), r.clone())).collect();
    let st: Map<String, Value> = reps.iter().map(|(n, r)| (n.clone(), json!(status(r)))).collect();
    Ok(json!({"clean": bad.is_empty(), "status": st,
              "problems": if bad.is_empty() { String::new() } else { render::terminal(&bad, &defs, &w, &IndexMap::new()) }}))
}

fn value_json(v: &V) -> Value {
    match v {
        V::Int(n) => json!(*n as i64),
        V::Bool(b) => json!(b),
        V::Str(s) => json!(s),
        V::None => Value::Null,
        V::Ctor(c, a) => {
            let mut xs = vec![json!(c)];
            xs.extend(a.iter().map(value_json));
            Value::Array(xs)
        }
    }
}

pub fn kern_run(file: &str, call: &str, world: &str) -> T<Value> {
    let (_, defs) = load(file)?;
    let pe = |e: crate::syntax::ParseError| ToolError("ParseError", e.0);
    // Text in the call or the world: parse both as an `eg` line does (quote-aware)
    let (mut w, name, args): (World, String, Vec<V>) = if call.contains('"') || world.contains('"') || call.contains('[') || world.contains('[') || !defs.account_states.is_empty() || !defs.decimal_states.is_empty() {
        let mut ex = crate::syntax::parse_call(world, call).map_err(pe)?;
        crate::syntax::normalize_example(&defs.decimal_states, &mut ex).map_err(pe)?;
        need_def(&defs, &ex.call.0)?;
        let args: Vec<V> = crate::run::example_args(&ex);
        (World::of_example(&ex), ex.call.0.clone(), args)
    } else {
        let (rest, now) = split_now(world).map_err(pe)?;
        let (rest, maps) = split_maps(&rest).map_err(pe)?;
        let w = World::from_example(&parse_world(&rest, None).map_err(pe)?, now, &maps);
        let mut parts = call.split_whitespace();
        let name = parts.next().unwrap_or("");
        need_def(&defs, name)?;
        let args: Vec<V> = parts
            .map(|x| {
                let digits = x.trim_start_matches('-');
                match x.parse::<crate::Int>() {
                    Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => V::Int(n),
                    _ => crate::syntax::decimal_value(x).map(crate::run::decimal).unwrap_or_else(|_| V::Str(x.to_string())),
                }
            })
            .collect();
        (w, name.to_string(), args)
    };
    let name = name.as_str();
    let mut interp = Interp::new(&defs);
    let outcome = interp.run(name, &args, &mut w).map_err(|e| ToolError("KernError", e.to_string()))?;
    // a definition with `-> Int` returns a value on success
    let returned = match (&outcome, defs.defs[name].ret.is_some(), &interp.last_result) {
        (Outcome::Ok, true, Some(v)) => value_json(v),
        _ => Value::Null,
    };
    let state: Map<String, Value> = w
        .maps
        .iter()
        .map(|(s, m)| {
            let kv: Map<String, Value> = m
                .iter()
                .map(|(k, v)| (match k { Key::Int(n) => n.to_string(), Key::Str(s) => s.clone() }, json!(*v as i64)))
                .collect();
            (s.clone(), Value::Object(kv))
        })
        .chain(w.texts.iter().map(|(s, m)| {
            let kv: Map<String, Value> =
                m.iter().map(|(k, v)| (match k { Key::Int(n) => n.to_string(), Key::Str(s) => s.clone() }, json!(v))).collect();
            (s.clone(), Value::Object(kv))
        }))
        .collect();
    Ok(json!({
        "outcome": match &outcome { Outcome::Ok => "ok".to_string(), o => o.text() },
        "balances": w.bal.iter().map(|(a, b)| (a.clone(), json!(*b as i64))).collect::<Map<String, Value>>(),
        "events": w.audit.iter().map(value_json).collect::<Vec<_>>(),
        "state": state,
        "result": returned,
    }))
}

pub fn kern_callers(file: &str, name: &str) -> T<Value> {
    let (_, defs) = load(file)?;
    need_def(&defs, name)?;
    let mut direct: Vec<&String> =
        defs.defs.iter().filter(|(_, d)| callees(&defs, d).iter().any(|c| c == name)).map(|(n, _)| n).collect();
    direct.sort();
    Ok(json!({"direct": direct, "transitive": callers(&defs, name).into_iter().collect::<Vec<_>>()}))
}

/// Python calls `fn(**arguments)`: unknown and missing keyword arguments are TypeErrors with Python's text.
fn args_for<'a>(tool_name: &str, args: &'a Map<String, Value>, params: &[(&str, bool)]) -> T<Vec<Option<&'a Value>>> {
    for k in args.keys() {
        if !params.iter().any(|(p, _)| p == k) {
            return Err(ToolError("TypeError", format!("{}() got an unexpected keyword argument {}", tool_name, repr_str(k))));
        }
    }
    let missing: Vec<String> =
        params.iter().filter(|(p, req)| *req && !args.contains_key(*p)).map(|(p, _)| repr_str(p)).collect();
    if !missing.is_empty() {
        let list = match missing.len() {
            1 => missing[0].clone(),
            _ => format!("{} and {}", missing[..missing.len() - 1].join(", "), missing[missing.len() - 1]),
        };
        return Err(ToolError(
            "TypeError",
            format!(
                "{}() missing {} required positional argument{}: {}",
                tool_name,
                missing.len(),
                if missing.len() == 1 { "" } else { "s" },
                list
            ),
        ));
    }
    Ok(params.iter().map(|(p, _)| args.get(*p)).collect())
}

fn s<'a>(v: Option<&'a Value>, what: &str) -> T<&'a str> {
    match v {
        Some(Value::String(x)) => Ok(x),
        _ => Err(ToolError("TypeError", format!("{} must be a string", what))),
    }
}

fn opt_s<'a>(v: Option<&'a Value>) -> Option<&'a str> {
    v.and_then(|x| x.as_str())
}

/// One tool call outside any session (nothing is stubbed).
pub fn call_tool(name: &str, args: &Map<String, Value>) -> Option<T<Value>> {
    call_tool_in(&mut Session::default(), name, args)
}

fn call_tool_in(session: &mut Session, name: &str, args: &Map<String, Value>) -> Option<T<Value>> {
    Some(match name {
        "kern_list" => args_for(name, args, &[("file", true)]).and_then(|a| kern_list(s(a[0], "file")?)),
        "kern_view" => args_for(name, args, &[("file", true), ("name", true), ("as", false), ("full", false)]).and_then(|a| {
            let full = a[3].map_or(false, |f| f.as_bool().unwrap_or(!f.is_null()));
            kern_view_in(session, s(a[0], "file")?, s(a[1], "name")?, opt_s(a[2]).unwrap_or("iface"), full)
        }),
        "kern_why" => {
            args_for(name, args, &[("file", true), ("name", true)]).and_then(|a| kern_why(s(a[0], "file")?, s(a[1], "name")?))
        }
        "kern_check" => args_for(name, args, &[("file", true), ("quick", false), ("name", false)]).and_then(|a| {
            let quick = a[1].map_or(true, |q| q.as_bool().unwrap_or(!q.is_null()));
            kern_check(s(a[0], "file")?, quick, opt_s(a[2]))
        }),
        "kern_run" => args_for(name, args, &[("file", true), ("call", true), ("world", true)])
            .and_then(|a| kern_run(s(a[0], "file")?, s(a[1], "call")?, s(a[2], "world")?)),
        "kern_callers" => args_for(name, args, &[("file", true), ("name", true)])
            .and_then(|a| kern_callers(s(a[0], "file")?, s(a[1], "name")?)),
        "kern_edit" => args_for(name, args, &[("file", true), ("ops", true)]).and_then(|a| {
            let ops = a[1].and_then(|o| o.as_array()).ok_or_else(|| ToolError("TypeError", "ops must be a list".into()))?;
            kern_edit(s(a[0], "file")?, ops)
        }),
        "kern_add" => args_for(name, args, &[("file", true), ("source", true)]).and_then(|a| {
            kern_edit(s(a[0], "file")?, &[json!({"op": "add", "source": a[1]})])
        }),
        "kern_replace" => {
            args_for(name, args, &[("file", true), ("name", true), ("source", true), ("expect_hash", false)]).and_then(|a| {
                kern_edit(
                    s(a[0], "file")?,
                    &[json!({"op": "replace", "name": a[1], "source": a[2], "expect_hash": a[3]})],
                )
            })
        }
        "kern_remove" => args_for(name, args, &[("file", true), ("name", true), ("expect_hash", false)]).and_then(|a| {
            kern_edit(s(a[0], "file")?, &[json!({"op": "remove", "name": a[1], "expect_hash": a[2]})])
        }),
        _ => return None,
    })
}

// --- JSON-RPC over stdio ---------------------------------------------------------------------

/// KERN_MCP_LOG=path: one JSON line per tool call with the tokens it returned (for C2).
fn log(tool: &str, args: &Map<String, Value>, text: &str) {
    let Ok(path) = std::env::var("KERN_MCP_LOG") else { return };
    if path.is_empty() {
        return;
    }
    let short: Map<String, Value> = args
        .iter()
        .map(|(k, v)| {
            let text = match v {
                Value::String(x) => x.clone(),
                other => pyjson::dumps(other, None),
            };
            if text.chars().count() < 200 {
                (k.clone(), v.clone())
            } else {
                (k.clone(), json!(format!("{}…", text.chars().take(200).collect::<String>())))
            }
        })
        .collect();
    let line = json!({"tool": tool, "args": short, "response_tokens": views::tokens(text), "response_chars": text.chars().count()});
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", pyjson::dumps(&line, None));
    }
}

/// One JSON-RPC message -> the response, or None for a notification, outside any session.
pub fn handle(msg: &Value) -> Option<Value> {
    handle_in(&mut Session::default(), msg)
}

fn handle_in(session: &mut Session, msg: &Value) -> Option<Value> {
    let mid = msg.get("id").filter(|v| !v.is_null())?.clone();
    let method = msg.get("method").and_then(|m| m.as_str());
    let empty = Map::new();
    let params = msg.get("params").and_then(|p| p.as_object()).unwrap_or(&empty);
    let result = match method {
        Some("initialize") => json!({
            "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!(PROTOCOL)),
            "capabilities": {"tools": {}}, "serverInfo": {"name": "kern", "version": "0.1"}}),
        Some("tools/list") => json!({ "tools": tools() }),
        Some("tools/call") => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").and_then(|a| a.as_object()).unwrap_or(&empty);
            match call_tool_in(session, name, args) {
                None => {
                    let shown = params.get("name").and_then(|n| n.as_str()).map(repr_str).unwrap_or_else(|| "None".into());
                    return Some(json!({"jsonrpc": "2.0", "id": mid,
                                       "error": {"code": -32602, "message": format!("unknown tool {}", shown)}}));
                }
                Some(Ok(out)) => {
                    let text = pyjson::dumps(&out, Some(1));
                    log(name, args, &text);
                    json!({"content": [{"type": "text", "text": text}], "isError": false})
                }
                Some(Err(ToolError(kind, m))) => {
                    json!({"content": [{"type": "text", "text": format!("{}: {}", kind, m)}], "isError": true})
                }
            }
        }
        Some("ping") => json!({}),
        other => {
            let shown = other.map(repr_str).unwrap_or_else(|| "None".into());
            return Some(json!({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": format!("no method {}", shown)}}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": mid, "result": result}))
}

/// The stdio loop.
pub fn serve(inp: impl BufRead, out: &mut dyn Write) -> std::io::Result<()> {
    let mut session = Session::default();
    for line in inp.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Err(_) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})),
            Ok(msg) => session.handle(&msg),
        };
        if let Some(r) = resp {
            writeln!(out, "{}", pyjson::dumps(&r, None))?;
            out.flush()?;
        }
    }
    Ok(())
}

pub fn main() -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    match serve(stdin.lock(), &mut stdout) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("kern mcp: {}", e);
            1
        }
    }
}
