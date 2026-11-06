//! Packages and imports (port of kern/package.py). Loading only: the files of a package merge into one Module, each
//! definition knowing its file, and `import a.b` controls what a file may use. The prover, interpreter and host see
//! one Module, as before.
//!
//! - A package is a directory with a `kern.pkg` file. Loading any file in it loads every `.k` file under it.
//! - A module is one file, named by its dotted path from the package root (`billing/invoice.k` is
//!   `billing.invoice`). Outside a package, a file loads itself plus what it imports, from its directory.
//! - A file may use only the definitions, state, handlers and channels it declares or imports directly. Names are
//!   unique across the package.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::infer::walk;
use crate::pyrepr::repr_str;
use crate::syntax::{expand_refs, flat, parse_module, Def, Emit, Expr, Module, Need, ParseError, Stmt};
use crate::wf::check_module;

pub const MARKER: &str = "kern.pkg";

/// What the loader knows about a package (Python's `defs.package`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Package {
    pub root: Option<String>,
    pub base: String,
    /// module -> file
    pub files: IndexMap<String, String>,
    /// module -> imported modules
    pub imports: IndexMap<String, Vec<String>>,
    /// kind ("def" / "state" / "handler" / "channel") -> name -> module
    pub origin: IndexMap<String, IndexMap<String, String>>,
}

fn name_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[a-z]\w*(?:\.[a-z]\w*)*$").unwrap())
}

fn load_error(path: &str, msg: &str, line: Option<usize>) -> ParseError {
    match line {
        Some(n) if n > 0 => ParseError(format!("{}:{}: {}", path, n, msg)),
        _ => ParseError(format!("{}: {}", path, msg)),
    }
}

/// Python's os.path.abspath: absolute and normalised (`.` and `..` resolved lexically, no symlinks followed).
pub fn abspath(p: &str) -> String {
    let joined = if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out: Vec<String> = vec![];
    for c in joined.components() {
        match c {
            std::path::Component::RootDir => out.clear(),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
            std::path::Component::Prefix(_) => {}
        }
    }
    format!("/{}", out.join("/"))
}

fn dirname(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

fn join(a: &str, b: &str) -> String {
    if a.ends_with('/') {
        format!("{}{}", a, b)
    } else {
        format!("{}/{}", a, b)
    }
}

/// Python's os.path.relpath for absolute paths.
pub fn relpath(path: &str, start: &str) -> String {
    let (pa, sa) = (abspath(path), abspath(start));
    let p: Vec<&str> = pa.split('/').filter(|s| !s.is_empty()).collect();
    let s: Vec<&str> = sa.split('/').filter(|s| !s.is_empty()).collect();
    let common = p.iter().zip(&s).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; s.len() - common];
    parts.extend(&p[common..]);
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

/// Read a file as Python's text mode does (\r\n and \r become \n).
pub fn read_text(path: &str) -> std::io::Result<String> {
    let t = std::fs::read_to_string(path)?;
    Ok(if t.contains('\r') { t.replace("\r\n", "\n").replace('\r', "\n") } else { t })
}

/// The package root: the nearest directory, from the file's up, holding a kern.pkg. None outside one.
pub fn root_of(path: &str) -> Option<String> {
    let a = abspath(path);
    let mut d = if Path::new(&a).is_dir() { a } else { dirname(&a) };
    loop {
        if Path::new(&join(&d, MARKER)).exists() {
            return Some(d);
        }
        let up = dirname(&d);
        if up == d || up.is_empty() {
            return None;
        }
        d = up;
    }
}

pub fn module_name(base: &str, path: &str) -> String {
    let r = relpath(path, base);
    r[..r.len() - ".k".len()].replace('/', ".")
}

pub fn module_path(base: &str, name: &str) -> String {
    format!("{}.k", join(base, &name.split('.').collect::<Vec<_>>().join("/")))
}

/// [(module, line)] and the text with the import lines blanked (line numbers stay right).
pub fn split_imports(text: &str, path: &str) -> Result<(Vec<(String, usize)>, String), ParseError> {
    let (mut imports, mut lines) = (vec![], vec![]);
    for (i, raw) in text.split('\n').enumerate() {
        let no = i + 1;
        if raw.starts_with("import ") || raw.trim() == "import" {
            let name = raw.get("import ".len()..).unwrap_or("").trim();
            if !name_re().is_match(name) {
                return Err(load_error(
                    path,
                    &format!(
                        "want `import a.b` (a module name, relative to the package root), got {}",
                        repr_str(raw)
                    ),
                    Some(no),
                ));
            }
            imports.push((name.to_string(), no));
            lines.push("");
        } else {
            lines.push(raw);
        }
    }
    Ok((imports, lines.join("\n")))
}

/// Whether loading this file needs the package loader (inside a package, or it imports).
pub fn is_package_file(path: &str, text: Option<&str>) -> bool {
    if root_of(path).is_some() {
        return true;
    }
    let owned;
    let text = match text {
        Some(t) => t,
        None => {
            owned = read_text(path).unwrap_or_default();
            &owned
        }
    };
    text.split('\n').any(|l| l.starts_with("import "))
}

fn walk_k_files(root: &str) -> Vec<String> {
    fn go(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                go(&p, root, out);
            } else if p.extension().map_or(false, |x| x == "k") {
                let rel = dir.strip_prefix(root).unwrap_or(Path::new(""));
                if rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
                    continue;
                }
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = vec![];
    go(Path::new(root), Path::new(root), &mut out);
    out.sort();
    out
}

/// The checked Module for the package (or import closure) `path` belongs to. overrides = {path: text} replaces a
/// file's text in memory (an edit being validated).
pub fn load(path: &str, overrides: &IndexMap<String, String>) -> Result<Module, ParseError> {
    let overrides: IndexMap<String, String> = overrides.iter().map(|(p, t)| (abspath(p), t.clone())).collect();
    let mut path = abspath(path);
    if Path::new(&path).is_dir() {
        // the package directory itself
        path = join(&path, MARKER);
    }
    let read = |p: &str| -> Result<String, ParseError> {
        match overrides.get(p) {
            Some(t) => Ok(t.clone()),
            None => read_text(p).map_err(|e| ParseError(io_error_text(&e, p))),
        }
    };
    let root = root_of(&path);
    if root.is_none() && !is_package_file(&path, Some(&read(&path)?)) {
        // a plain single-file module, exactly as before
        let defs = parse_module(&read(&path)?, true)?;
        check_module(&defs)?;
        return Ok(defs);
    }
    let base = root.clone().unwrap_or_else(|| dirname(&path));
    // packages this one uses (`use ../money` in kern.pkg), transitively: their modules are `money.<module>`
    let deps = match &root {
        Some(r) => dependencies(r)?,
        None => vec![],
    };
    // module -> (file, the prefix its own imports may leave out: "" here, "money." in a dependency)
    let mut known: IndexMap<String, (String, String)> = IndexMap::new();
    if let Some(r) = &root {
        for p in walk_k_files(r) {
            known.insert(module_name(&base, &p), (p, String::new()));
        }
        for (name, dir) in &deps {
            for p in walk_k_files(dir) {
                let m = format!("{}.{}", name, module_name(dir, &p));
                if let Some((other, _)) = known.get(&m) {
                    return Err(load_error(&p, &format!("module {} is also {}", m, relpath(other, &base)), None));
                }
                known.insert(m, (p, format!("{name}.")));
            }
        }
    }
    let mut todo: Vec<(String, String, String)> = match &root {
        Some(_) => known.iter().map(|(m, (p, pre))| (p.clone(), m.clone(), pre.clone())).collect(),
        None => vec![(path.clone(), module_name(&base, &path), String::new())],
    };
    let mut files: IndexMap<String, (String, Module, String)> = IndexMap::new();
    let mut imports: IndexMap<String, Vec<(String, usize)>> = IndexMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while !todo.is_empty() {
        let (p, m, prefix) = todo.remove(0);
        if !seen.insert(p.clone()) {
            continue;
        }
        let text = read(&p)?;
        let (imps, stripped) = split_imports(&text, &p)?;
        let defs = parse_module(&stripped, false).map_err(|e| load_error(&p, &e.0, None))?;
        let mut resolved = vec![];
        for (name, no) in &imps {
            if root.is_some() {
                // within a dependency, its own modules first, then another package's
                let full = format!("{prefix}{name}");
                let name = if !prefix.is_empty() && known.contains_key(&full) { full } else { name.clone() };
                if !known.contains_key(&name) {
                    let target = module_path(&base, &name);
                    if !overrides.contains_key(&target) {
                        return Err(load_error(
                            &p,
                            &format!("no module {} (looked for {})", name, relpath(&target, &base)),
                            Some(*no),
                        ));
                    }
                }
                resolved.push((name, *no));
                continue;
            }
            let target = module_path(&base, name);
            if !Path::new(&target).exists() && !overrides.contains_key(&target) {
                return Err(load_error(
                    &p,
                    &format!("no module {} (looked for {})", name, relpath(&target, &base)),
                    Some(*no),
                ));
            }
            todo.push((target.clone(), module_name(&base, &target), String::new()));
            resolved.push((name.clone(), *no));
        }
        files.insert(m.clone(), (p, defs, text));
        imports.insert(m, resolved);
    }
    let mut merged = Module::default();
    let mut origin: IndexMap<String, IndexMap<String, String>> =
        ["def", "state", "handler", "channel"].iter().map(|k| (k.to_string(), IndexMap::new())).collect();
    let mut mods: Vec<String> = files.keys().cloned().collect();
    mods.sort();
    // named constants: unique across the package, substituted per file from what that file declares or imports
    let mut const_origin: IndexMap<String, String> = IndexMap::new();
    for m in &mods {
        let (p, defs, _) = &files[m];
        for (n, v) in &defs.consts {
            if let Some(other) = const_origin.get(n) {
                return Err(load_error(p, &format!("const {} is already declared in {}", n, other), None));
            }
            const_origin.insert(n.clone(), m.clone());
            merged.consts.insert(n.clone(), *v);
        }
    }
    for m in &mods {
        let mut visible_mods: BTreeSet<&str> = imports[m].iter().map(|(n, _)| n.as_str()).collect();
        visible_mods.insert(m);
        let (mut visible, mut hidden) = (IndexMap::new(), IndexMap::new());
        for (n, owner) in &const_origin {
            if visible_mods.contains(owner.as_str()) {
                visible.insert(n.clone(), merged.consts[n]);
            } else {
                hidden.insert(n.clone(), owner.clone());
            }
        }
        let (p, defs, _) = files.get_mut(m).unwrap();
        crate::syntax::resolve_consts(defs, &visible, &hidden).map_err(|e| load_error(p, &e.0, None))?;
    }
    for m in &mods {
        let (p, defs, text) = files.get_mut(m).unwrap();
        let kinds: [(&str, Vec<String>); 3] = [
            ("state", defs.states.keys().cloned().collect()),
            ("handler", defs.handlers.keys().cloned().collect()),
            ("channel", defs.channels.clone()),
        ];
        for (kind, names) in kinds {
            for n in names {
                if let Some(other) = origin[kind].get(&n) {
                    return Err(load_error(p, &format!("{} {} is already declared in {}", kind, n, other), None));
                }
                origin.get_mut(kind).unwrap().insert(n, m.clone());
            }
        }
        merged.states.extend(defs.states.iter().map(|(a, b)| (a.clone(), b.clone())));
        merged.record_states.extend(defs.record_states.iter().map(|(a, b)| (a.clone(), b.clone())));
        merged.account_states.extend(defs.account_states.iter().cloned());
        merged.list_states.extend(defs.list_states.iter().cloned());
        merged.decimal_states.extend(defs.decimal_states.iter().cloned());
        merged.channels.extend(defs.channels.iter().cloned());
        merged.invariants.extend(defs.invariants.iter().cloned());
        for d in defs.defs.values_mut().chain(defs.handlers.values_mut()) {
            d.module = Some(m.clone());
            d.file = Some(p.clone());
            d.line = header_line(text, d);
        }
        merged.handlers.extend(defs.handlers.iter().map(|(a, b)| (a.clone(), b.clone())));
        for (n, d) in &defs.defs {
            if merged.defs.contains_key(n) || origin["handler"].contains_key(n) {
                let other = origin["def"].get(n).or_else(|| origin["handler"].get(n)).cloned().unwrap_or_default();
                return Err(load_error(p, &format!("{} is already defined in {}", n, other), d.line));
            }
            merged.defs.insert(n.clone(), d.clone());
            origin.get_mut("def").unwrap().insert(n.clone(), m.clone());
        }
    }
    // records: unique across the package; a definition may name only those its file declares or imports
    let mut record_origin: IndexMap<String, String> = IndexMap::new();
    for m in &mods {
        let (p, defs, _) = &files[m];
        for (r, fields) in &defs.records {
            if let Some(other) = record_origin.get(r) {
                return Err(load_error(p, &format!("record {} is already declared in {}", r, other), None));
            }
            record_origin.insert(r.clone(), m.clone());
            merged.records.insert(r.clone(), fields.clone());
        }
    }
    let record_uses = crate::syntax::flatten_records(&mut merged).map_err(|e| load_error(&path, &e.0, None))?;
    crate::syntax::normalize_decimals(&mut merged).map_err(|e| load_error(&path, &e.0, None))?;
    // a stored record's field states (`order.qty`) belong to the module that declares `order`
    for x in merged.record_states.keys() {
        let owner = origin["state"].get(x).cloned();
        let prefix = format!("{x}.");
        for f in merged.states.keys().filter(|f| f.starts_with(&prefix)) {
            if let Some(o) = &owner {
                origin.get_mut("state").unwrap().insert(f.clone(), o.clone());
            }
        }
    }
    for (name, rs) in &record_uses {
        let d = &merged.defs[name];
        let m = d.module.clone().unwrap_or_default();
        for r in rs {
            let owner = &record_origin[r];
            if *owner != m && !imports[&m].iter().any(|(i, _)| i == owner) {
                let p = d.file.clone().unwrap_or_default();
                return Err(load_error(
                    &p,
                    &format!("{} uses record {}, which {} declares: add `import {}`", name, r, owner, owner),
                    d.line,
                ));
            }
        }
    }
    for (n, owner) in &const_origin {
        let kind = ["def", "state", "handler", "channel"].into_iter().find(|k| origin[*k].contains_key(n));
        if let Some(kind) = kind {
            let p = &files[owner].0;
            return Err(load_error(p, &format!("const {} has the name of a {} in {}", n, kind, origin[kind][n]), None));
        }
    }
    expand_refs(&mut merged).map_err(|e| load_error(&path, &e.0, None))?;
    for (m, (p, defs, _)) in &files {
        let mut visible: BTreeSet<&str> = imports[m].iter().map(|(n, _)| n.as_str()).collect();
        visible.insert(m);
        for name in defs.defs.keys() {
            let d = &merged.defs[name];
            for (kind, n) in uses_of(&merged, d) {
                if let Some(owner) = origin[kind.as_str()].get(&n) {
                    if !visible.contains(owner.as_str()) {
                        return Err(load_error(
                            p,
                            &format!("{} uses {} {}, which {} declares: add `import {}`", d.name, kind, n, owner, owner),
                            d.line,
                        ));
                    }
                }
            }
        }
    }
    merged.package = Some(Package {
        root,
        base,
        files: files.iter().map(|(m, f)| (m.clone(), f.0.clone())).collect(),
        imports: imports.iter().map(|(m, i)| (m.clone(), i.iter().map(|(n, _)| n.clone()).collect())).collect(),
        origin,
    });
    check_module(&merged)?;
    Ok(merged)
}

/// The package roots that use `path`'s own package (directly or through another): every directory with a kern.pkg
/// under `within` (searched down to 12 levels; .git, target, node_modules, build and .kern-cache are skipped) or
/// next to the package's root, whose dependencies include it. A user elsewhere on disk isn't found. Empty outside a
/// package.
pub fn users_of(path: &str, within: &str) -> Vec<String> {
    let Some(root) = root_of(path) else { return vec![] };
    let mut found: Vec<String> = vec![];
    package_dirs(&abspath(within), 12, &mut found);
    if let Ok(rd) = std::fs::read_dir(dirname(&root)) {
        found.extend(rd.flatten().map(|e| e.path().to_string_lossy().into_owned()).filter(|d| Path::new(&join(d, MARKER)).exists()));
    }
    let mut out: Vec<String> = found
        .into_iter()
        .map(|d| abspath(&d))
        .filter(|d| *d != root)
        .filter(|d| dependencies(d).is_ok_and(|ds| ds.iter().any(|(_, r)| *r == root)))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn package_dirs(dir: &str, depth: u32, out: &mut Vec<String>) {
    if Path::new(&join(dir, MARKER)).exists() {
        out.push(dir.to_string());
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    if depth == 0 {
        return;
    }
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if ["target", "node_modules", "build", ".git", ".kern-cache"].contains(&name.as_str()) {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            package_dirs(&join(dir, &name), depth - 1, out);
        }
    }
}

/// The packages `root` uses, transitively: (name, directory). kern.pkg holds `use PATH` lines (relative to the
/// package), each naming a directory with its own kern.pkg; the name is the directory's. Blank lines are allowed.
pub fn dependencies(root: &str) -> Result<Vec<(String, String)>, ParseError> {
    let mut out: Vec<(String, String)> = vec![];
    let mut todo = vec![(root.to_string(), vec![abspath(root)])];
    while let Some((dir, chain)) = todo.pop() {
        let file = join(&dir, MARKER);
        let text = read_text(&file).unwrap_or_default();
        for (i, line) in text.split('\n').enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(rel) = line.strip_prefix("use ").map(str::trim) else {
                return Err(load_error(&file, &format!("want `use ../other` (a package directory), got {}", repr_str(line)), Some(i + 1)));
            };
            let target = if rel.starts_with('/') { abspath(rel) } else { abspath(&join(&dir, rel)) };
            if !Path::new(&join(&target, MARKER)).exists() {
                return Err(load_error(&file, &format!("{} is not a package (no {} in it)", rel, MARKER), Some(i + 1)));
            }
            let name = target.rsplit('/').next().unwrap_or("").to_string();
            if !name_re().is_match(&name) || name.contains('.') {
                return Err(load_error(&file, &format!("a package used by name {} needs a lower-case name", repr_str(&name)), Some(i + 1)));
            }
            if chain.contains(&target) {
                return Err(load_error(&file, &format!("packages use each other in a cycle through {}", name), Some(i + 1)));
            }
            if text.lines().filter(|l| l.trim() == line).count() > 1 {
                return Err(load_error(&file, &format!("`{}` appears twice", line), Some(i + 1)));
            }
            match out.iter().find(|(n, _)| *n == name) {
                Some((_, d)) if *d == target => continue,
                Some((_, d)) => {
                    return Err(load_error(&file, &format!("two packages are named {}: {} and {}", name, d, target), Some(i + 1)))
                }
                None => {}
            }
            out.push((name, target.clone()));
            let mut c = chain.clone();
            c.push(target.clone());
            todo.push((target, c));
        }
    }
    Ok(out)
}

fn io_error_text(e: &std::io::Error, p: &str) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("[Errno 2] No such file or directory: {}", repr_str(p)),
        _ => format!("{}: {}", p, e),
    }
}

fn header_line(text: &str, d: &Def) -> Option<usize> {
    let head = d.src_lines.get("header").and_then(|h| h.first()).cloned().unwrap_or_default();
    text.split('\n').position(|raw| raw.trim_end() == head).map(|i| i + 1)
}

/// (kind, name) for every definition, state, handler and channel d's spec or body names, sorted.
fn uses_of(defs: &Module, d: &Def) -> Vec<(String, String)> {
    let mut exprs: Vec<&Expr> = vec![];
    for n in &d.needs {
        if let Need::Need(c, _, _) = n {
            exprs.push(c);
        }
    }
    for n in &d.needs {
        if let Need::Need(_, _, a) = n {
            exprs.extend(a.iter());
        }
    }
    exprs.extend(d.keeps.iter());
    exprs.extend(d.ensures.iter());
    for ev in &d.emits {
        let (Emit::Ev(_, args, cond) | Emit::As(_, args, _, cond)) = ev;
        exprs.extend(args.iter());
        exprs.extend(cond.iter());
    }
    for s in flat(&d.body) {
        match s {
            Stmt::For(_, lo, hi, _, _) => exprs.extend([lo, hi]),
            Stmt::Inv(_) => {}
            Stmt::Let(_, e) | Stmt::Do(e) | Stmt::If(e, _, _) => exprs.push(e),
            Stmt::Need(c, _, a) => {
                exprs.push(c);
                exprs.extend(a.iter());
            }
        }
    }
    let mut out: BTreeSet<(String, String)> = d.ref_origin.values().map(|c| ("def".to_string(), c.clone())).collect();
    for e in exprs {
        for n in walk(e) {
            let name = match n {
                Expr::App(h, _, _) => h,
                Expr::Name(x, _) => x,
                // `keep X …` names a state
                Expr::Frame(x, _) if defs.states.contains_key(x) => {
                    out.insert(("state".into(), x.clone()));
                    continue;
                }
                _ => continue,
            };
            let (x, op) = name.rsplit_once('.').unwrap_or(("", name.as_str()));
            if defs.defs.contains_key(name) && *name != d.name {
                out.insert(("def".into(), name.clone()));
            } else if defs.handlers.contains_key(name) {
                out.insert(("handler".into(), name.clone()));
            } else if defs.states.contains_key(x) && (op == "get" || op == "set") {
                out.insert(("state".into(), x.to_string()));
            } else if defs.channels.iter().any(|c| c == x) && op == "emit" {
                out.insert(("channel".into(), x.to_string()));
            }
        }
    }
    out.into_iter().collect()
}

/// Where the package's .why / .strength lives: <root>/kern.<ext> in a package, else next to the file.
pub fn store_path(path: &str, ext: &str) -> String {
    match root_of(path) {
        Some(r) => join(&r, &format!("kern{}", ext)),
        None => {
            // os.path.splitext: the extension starts at the last dot of the last component (not a leading dot)
            let (dir, file) = match path.rfind('/') {
                Some(i) => (&path[..=i], &path[i + 1..]),
                None => ("", path),
            };
            let stem = match file.trim_start_matches('.').rfind('.') {
                Some(i) => &file[..i + (file.len() - file.trim_start_matches('.').len())],
                None => file,
            };
            format!("{}{}{}", dir, stem, ext)
        }
    }
}

/// {module: path} of a loaded package, or {} for a single file.
pub fn files_of(defs: &Module) -> IndexMap<String, String> {
    defs.package.as_ref().map(|p| p.files.clone()).unwrap_or_default()
}
