//! Lazy `why` store (port of kern/why.py). Entries live outside the code, anchored to the hash of the definition
//! they explain; when the definition changes, its entries are reported as stale.
//!
//! File format:
//!     transfer@cd25dc69
//!       entry text
//!       another entry
//!
//! The anchor after `@` is the interface anchor (`anchor_hash`), not the build hash `check` prints; `kern anchor
//! FILE` writes it. Any token is accepted as an anchor (a placeholder is simply stale until anchored), but a line at
//! column 0 that isn't `name@anchor` is an error, so no entry is ever dropped or attached to the wrong definition.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::infer::{fails, fmt_fails, fmt_uses, lifted, uses, writes};
use crate::pyrepr::{repr, repr_str, sha_hex, Py};
use crate::syntax::{code, Def, Module};

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub hash: String,
    pub entries: Vec<String>,
}

pub type Store = IndexMap<String, Block>;

#[derive(Clone, Debug, PartialEq)]
pub struct WhyFileError(pub String);

impl std::fmt::Display for WhyFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WhyFileError {}

fn head() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(\w+)@(\S+)$").unwrap())
}

fn obligation() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)\b(must|never|always|only|required|do not|don't|mustn't|shall)\b").unwrap())
}

/// Parse a .why file's text (`path` is only used in error messages).
pub fn parse(text: &str, path: &str) -> Result<Store, WhyFileError> {
    let mut store = Store::new();
    let mut cur: Option<String> = None;
    // Python reads the file in text mode: \r\n and \r become \n, and lines keep their \n
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let no = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        if !line.chars().next().unwrap().is_whitespace() {
            let Some(m) = head().captures(line.trim()) else {
                return Err(WhyFileError(format!(
                    "{}:{}: want `name@anchor` at column 0, got {}",
                    path,
                    no,
                    repr_str(line.trim())
                )));
            };
            let name = m[1].to_string();
            store.entry(name.clone()).or_insert_with(|| Block { hash: m[2].to_string(), entries: vec![] });
            cur = Some(name);
        } else if let Some(c) = &cur {
            store.get_mut(c).unwrap().entries.push(line.trim().to_string());
        } else {
            return Err(WhyFileError(format!("{}:{}: an entry before any `name@anchor` line", path, no)));
        }
    }
    Ok(store)
}

/// Load a .why file; a missing file is an empty store.
pub fn load(path: &str) -> Result<Store, WhyFileError> {
    if path.is_empty() {
        return Ok(Store::new());
    }
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, path),
        Err(_) => Ok(Store::new()),
    }
}

/// What a why entry is anchored to: the definition's own text *and* its inferred interface (uses, fails, writes,
/// lifted refusal conditions). A callee change that alters the caller's interface therefore marks the caller's why
/// stale, even though the caller's text is unchanged.
pub fn anchor_hash(defs: &Module, d: &Def) -> String {
    let iface = Py::Tuple(vec![
        Py::Str(d.hash()),
        Py::Str(fmt_uses(&uses(defs, d))),
        Py::Str(fmt_fails(&fails(defs, d))),
        Py::List(writes(defs, d).iter().map(|w| Py::s(w)).collect()),
        Py::List(lifted(defs, d).iter().map(|l| Py::Tuple(vec![Py::Str(code(&l.cond)), Py::s(&l.ctor)])).collect()),
    ]);
    sha_hex(&repr(&iface), 8)
}

/// A definition's why status, as the views and the page show it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub count: usize,
    pub stale: bool,
    pub entries: Vec<String>,
    pub anchored_to: Option<String>,
    pub current: Option<String>,
    /// rule 3: nothing that must be obeyed may live only in prose (heuristic, a warning)
    pub obligations: Vec<String>,
}

pub fn status(store: &Store, d: &Def, defs: &Module) -> Status {
    let Some(s) = store.get(&d.name) else { return Status::default() };
    let current = anchor_hash(defs, d);
    Status {
        count: s.entries.len(),
        stale: s.hash != current,
        entries: s.entries.clone(),
        anchored_to: Some(s.hash.clone()),
        current: Some(current),
        obligations: s.entries.iter().filter(|e| obligation().is_match(e)).cloned().collect(),
    }
}

/// Why entries whose definition no longer exists (renamed or deleted).
pub fn orphans(store: &Store, defs: &Module) -> IndexMap<String, Vec<String>> {
    store
        .iter()
        .filter(|(n, _)| !defs.defs.contains_key(*n))
        .map(|(n, s)| (n.clone(), s.entries.clone()))
        .collect()
}

/// The file text with every entry re-anchored to the current interface. Orphans keep their anchor.
pub fn anchored_text(store: &Store, defs: &Module) -> String {
    let mut out = String::new();
    for (name, s) in store {
        let h = match defs.defs.get(name) {
            Some(d) => anchor_hash(defs, d),
            None => s.hash.clone(),
        };
        out.push_str(&format!("{}@{}\n", name, h));
        for e in &s.entries {
            out.push_str(&format!("  {}\n", e));
        }
    }
    out
}

/// Re-anchor every entry to the current interface. Orphans are kept (never silently deleted) and returned, so the
/// caller can warn about them.
pub fn anchor(path: &str, defs: &Module) -> Result<IndexMap<String, Vec<String>>, Box<dyn std::error::Error>> {
    let store = load(path)?;
    std::fs::write(path, anchored_text(&store, defs))?;
    Ok(orphans(&store, defs))
}
