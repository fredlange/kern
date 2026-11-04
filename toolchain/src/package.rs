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
    let mut todo: Vec<String> = match &root {
        Some(r) => walk_k_files(r),
        None => vec![path.clone()],
    };
    let mut files: IndexMap<String, (String, Module, String)> = IndexMap::new();
    let mut imports: IndexMap<String, Vec<(String, usize)>> = IndexMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while !todo.is_empty() {
        let p = todo.remove(0);
        if !seen.insert(p.clone()) {
            continue;
        }
        let m = module_name(&base, &p);
        let text = read(&p)?;
        let (imps, stripped) = split_imports(&text, &p)?;
        let defs = parse_module(&stripped, false).map_err(|e| load_error(&p, &e.0, None))?;
        for (name, no) in &imps {
            let target = module_path(&base, name);
            if !Path::new(&target).exists() && !overrides.contains_key(&target) {
                return Err(load_error(
                    &p,
                    &format!("no module {} (looked for {})", name, relpath(&target, &base)),
                    Some(*no),
                ));
            }
            if root.is_none() {
                todo.push(target);
            }
        }
        files.insert(m.clone(), (p, defs, text));
        imports.insert(m, imps);
    }
    let mut merged = Module::default();
    let mut origin: IndexMap<String, IndexMap<String, String>> =
        ["def", "state", "handler", "channel"].iter().map(|k| (k.to_string(), IndexMap::new())).collect();
    let mut mods: Vec<String> = files.keys().cloned().collect();
    mods.sort();
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
