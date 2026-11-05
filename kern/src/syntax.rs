//! Lexer and parser (port of kern/syntax.py; that file's docstring has the grammar).
//!
//! The AST mirrors the Python tuples one to one, so `to_py()` gives back exactly the value Python hashes:
//!     ('int', n) ('name', s, primed) ('bin', op, l, r) ('app', (head, primed), [args]) ('all', j, A, B, e)
//!     statements: ('let', x, e) ('need', cond, ctor, [args]) ('do', e) ('if', c, [then], [else])
//!                 ('for', k, lo, hi, [invariants], [body]) ('inv', clause)
//!
//! Deliberate divergences from Python, where Python crashes with an uncaught exception instead of a ParseError:
//! an `eg` with two `->`, an empty call or outcome, a non-integer outcome argument or `--5` as an argument, and
//! integer literals beyond i128. Digits are ASCII only (Python's `\d` also lexes other Unicode digits).

use std::fmt;

use indexmap::IndexMap;
use regex::Regex;
use std::sync::OnceLock;

use crate::pyrepr::{repr, repr_str, sha_hex, Py};
use crate::Int;

// ---------------------------------------------------------------- AST

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Expr {
    Int(Int),
    /// a Text literal `"…"`
    Text(String),
    Name(String, bool),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    App(String, bool, Vec<Expr>),
    /// `E for j from A to B` (ensure and loop-invariant clauses only)
    All(String, Box<Expr>, Box<Expr>, Box<Expr>),
    /// `sum j from A to B of E [where C]`: E summed over j in [A, B), only where C holds
    Sum(String, Box<Expr>, Box<Expr>, Box<Expr>, Option<Box<Expr>>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Stmt {
    Let(String, Expr),
    Need(Expr, String, Vec<Expr>),
    Do(Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    For(String, Expr, Expr, Vec<Expr>, Vec<Stmt>),
    Inv(Expr),
}

/// A spec need: written, or `need as callee ARGS` before expansion.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    Need(Expr, String, Vec<Expr>),
    As(String, Vec<Expr>, usize),
}

/// A spec emit: `Ctor ARGS [when C]`, or `emit as callee ARGS [when C]` before expansion.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Emit {
    Ev(String, Vec<Expr>, Option<Expr>),
    As(String, Vec<Expr>, usize, Option<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Int(Int),
    Str(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Fail(String, Vec<Int>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    pub world: IndexMap<String, (Int, String)>,
    pub call: (String, Vec<Key>),
    pub outcome: Outcome,
    pub after: IndexMap<String, Int>,
    pub src: String,
    pub now: Int,
    pub maps: IndexMap<String, IndexMap<Key, Int>>,
    pub after_maps: IndexMap<String, IndexMap<Key, Int>>,
    /// Text state entries (`title.1:"hi"`), before and after
    pub texts: Texts,
    pub after_texts: Texts,
    /// the fourth section, when written: (channel, constructor, args) in order; `audit` is always compared then,
    /// other channels only if named
    pub events: Option<Vec<(String, String, Vec<Key>)>>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Def {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub ret: Option<String>,
    pub needs: Vec<Need>,
    pub keeps: Vec<Expr>,
    pub ensures: Vec<Expr>,
    pub emits: Vec<Emit>,
    pub complete: bool,
    /// (kind, index) -> callee, for `need as` / `emit as`
    pub ref_origin: IndexMap<(String, usize), String>,
    /// emit index -> channel; missing = `audit`
    pub emit_channel: IndexMap<usize, String>,
    pub examples: Vec<Example>,
    pub body: Vec<Stmt>,
    /// section -> source lines
    pub src_lines: IndexMap<String, Vec<String>>,
    /// a `handler` with an assumed spec and no body
    pub assumed: bool,
    /// set by the package loader: the module (dotted path), its file and the header's line
    pub module: Option<String>,
    pub file: Option<String>,
    pub line: Option<usize>,
    /// the named constants (`const NAME N`) the spec / body source mentions, with their values, for views
    pub spec_consts: Vec<(String, Int)>,
    pub body_consts: Vec<(String, Int)>,
    /// the record types (`record Point x:Int y:Int`) of the header's parameters, as declared, for views; the
    /// parameters themselves are flattened to their fields (`p:Point` -> `p.x:Int p.y:Int`)
    pub records: Vec<(String, Vec<(String, String)>)>,
    raw: Vec<(usize, String, usize)>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Module {
    pub defs: IndexMap<String, Def>,
    pub states: IndexMap<String, String>,
    pub invariants: Vec<(String, String, Expr)>,
    pub channels: Vec<String>,
    pub handlers: IndexMap<String, Def>,
    /// `const NAME N`: substituted into every use at load time (`resolve_consts`), kept for views and checks
    pub consts: IndexMap<String, Int>,
    /// `record NAME f:Type ...`: record-typed parameters are flattened to their fields at load time (`flatten_records`)
    pub records: IndexMap<String, Vec<(String, String)>>,
    /// declared states whose values are Text (`state NAME KEY -> Text`); the others hold Int
    pub text_states: Vec<String>,
    /// set by the package loader (kern/package.py's `defs.package`)
    pub package: Option<crate::package::Package>,
}

impl Module {
    /// The module with `d` replacing the definition of the same name, keeping the declarations.
    pub fn with_def(&self, d: Def) -> Module {
        let mut m = self.clone();
        m.defs.insert(d.name.clone(), d);
        m
    }
    pub fn get(&self, name: &str) -> Option<&Def> {
        self.defs.get(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl ParseError {
    pub fn at(msg: impl Into<String>, line: Option<usize>) -> ParseError {
        let msg = msg.into();
        match line {
            Some(n) if n > 0 => ParseError(format!("line {}: {}", n, msg)),
            _ => ParseError(msg),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

type PResult<T> = Result<T, ParseError>;

// ---------------------------------------------------------------- Python values (hashing, oracle dumps)

impl Expr {
    pub fn to_py(&self) -> Py {
        match self {
            Expr::Int(n) => Py::Tuple(vec![Py::s("int"), Py::Int(*n)]),
            Expr::Text(t) => Py::Tuple(vec![Py::s("text"), Py::s(t)]),
            Expr::Name(s, p) => Py::Tuple(vec![Py::s("name"), Py::s(s), Py::Bool(*p)]),
            Expr::Bin(op, l, r) => Py::Tuple(vec![Py::s("bin"), Py::s(op), l.to_py(), r.to_py()]),
            Expr::App(h, p, args) => Py::Tuple(vec![
                Py::s("app"),
                Py::Tuple(vec![Py::s(h), Py::Bool(*p)]),
                Py::List(args.iter().map(Expr::to_py).collect()),
            ]),
            Expr::All(j, a, b, e) => Py::Tuple(vec![Py::s("all"), Py::s(j), a.to_py(), b.to_py(), e.to_py()]),
            Expr::Sum(j, a, b, e, c) => Py::Tuple(vec![
                Py::s("sum"),
                Py::s(j),
                a.to_py(),
                b.to_py(),
                e.to_py(),
                c.as_ref().map(|c| c.to_py()).unwrap_or(Py::None),
            ]),
        }
    }
}

fn py_list<T>(xs: &[T], f: impl Fn(&T) -> Py) -> Py {
    Py::List(xs.iter().map(f).collect())
}

impl Stmt {
    pub fn to_py(&self) -> Py {
        match self {
            Stmt::Let(x, e) => Py::Tuple(vec![Py::s("let"), Py::s(x), e.to_py()]),
            Stmt::Need(c, ctor, args) => {
                Py::Tuple(vec![Py::s("need"), c.to_py(), Py::s(ctor), py_list(args, Expr::to_py)])
            }
            Stmt::Do(e) => Py::Tuple(vec![Py::s("do"), e.to_py()]),
            Stmt::If(c, t, o) => {
                Py::Tuple(vec![Py::s("if"), c.to_py(), py_list(t, Stmt::to_py), py_list(o, Stmt::to_py)])
            }
            Stmt::For(k, lo, hi, invs, body) => Py::Tuple(vec![
                Py::s("for"),
                Py::s(k),
                lo.to_py(),
                hi.to_py(),
                py_list(invs, Expr::to_py),
                py_list(body, Stmt::to_py),
            ]),
            Stmt::Inv(e) => Py::Tuple(vec![Py::s("inv"), e.to_py()]),
        }
    }
}

impl Need {
    pub fn to_py(&self) -> Py {
        match self {
            Need::Need(c, ctor, args) => {
                Py::Tuple(vec![Py::s("need"), c.to_py(), Py::s(ctor), py_list(args, Expr::to_py)])
            }
            Need::As(callee, args, no) => {
                Py::Tuple(vec![Py::s("need_as"), Py::s(callee), py_list(args, Expr::to_py), Py::Int(*no as Int)])
            }
        }
    }
}

impl Emit {
    pub fn to_py(&self) -> Py {
        match self {
            Emit::Ev(ctor, args, None) => Py::Tuple(vec![Py::s(ctor), py_list(args, Expr::to_py)]),
            Emit::Ev(ctor, args, Some(c)) => Py::Tuple(vec![Py::s(ctor), py_list(args, Expr::to_py), c.to_py()]),
            Emit::As(callee, args, no, cond) => Py::Tuple(vec![
                Py::s("emit_as"),
                Py::s(callee),
                py_list(args, Expr::to_py),
                Py::Int(*no as Int),
                cond.as_ref().map(Expr::to_py).unwrap_or(Py::None),
            ]),
        }
    }
    /// The condition of a written emit (`emit_cond` in Python).
    pub fn cond(&self) -> Option<&Expr> {
        match self {
            Emit::Ev(_, _, c) | Emit::As(_, _, _, c) => c.as_ref(),
        }
    }
}

impl Key {
    pub fn to_py(&self) -> Py {
        match self {
            Key::Int(n) => Py::Int(*n),
            Key::Str(s) => Py::s(s),
        }
    }
}

impl Def {
    pub fn new(name: &str, params: Vec<(String, String)>, ret: Option<String>) -> Def {
        Def { name: name.to_string(), params, ret, ..Default::default() }
    }

    /// The build hash: interface, spec and body, not examples or why (kern/syntax.py `Def.hash`).
    pub fn hash(&self) -> String {
        let params = Py::List(self.params.iter().map(|(n, t)| Py::Tuple(vec![Py::s(n), Py::s(t)])).collect());
        let ret = self.ret.as_deref().map(Py::s).unwrap_or(Py::None);
        let mut parts = vec![
            params,
            ret,
            py_list(&self.needs, Need::to_py),
            py_list(&self.keeps, Expr::to_py),
            py_list(&self.ensures, Expr::to_py),
            py_list(&self.body, Stmt::to_py),
        ];
        if !self.emits.is_empty() {
            parts.push(py_list(&self.emits, Emit::to_py));
        }
        if self.complete {
            parts.push(Py::s("ok otherwise"));
        }
        if !self.emit_channel.is_empty() {
            let mut items: Vec<_> = self.emit_channel.iter().collect();
            items.sort();
            parts.push(Py::List(
                items.into_iter().map(|(i, c)| Py::Tuple(vec![Py::Int(*i as Int), Py::s(c)])).collect(),
            ));
        }
        sha_hex(&repr(&Py::Tuple(parts)), 8)
    }

    /// Everything, examples included: the key for a content-addressed store.
    pub fn content_hash(&self) -> String {
        let srcs = Py::List(self.examples.iter().map(|e| Py::s(&e.src)).collect());
        sha_hex(&repr(&Py::Tuple(vec![Py::Str(self.hash()), srcs])), 8)
    }
}

// ---------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(Int),
    Text(String),
    Name(String),
    Op(&'static str),
}

const OPS: [&str; 14] = ["!=", ">=", "<=", "->", "=", "<", ">", "+", "-", "*", "/", "(", ")", "when"];
const CMP: [&str; 6] = ["=", "!=", ">=", "<=", ">", "<"];
const KEYWORDS: [&str; 5] = ["and", "or", "else", "when", "where"];

/// The interned operator for `s` (binary operators are &'static str in the AST).
pub fn op(s: &str) -> &'static str {
    for o in OPS.iter().chain(["and", "or"].iter()) {
        if *o == s {
            return o;
        }
    }
    panic!("not an operator: {}", s)
}

fn tok_repr(t: Option<&Tok>) -> String {
    match t {
        None => "None".into(),
        Some(Tok::Int(n)) => n.to_string(),
        Some(Tok::Text(t)) => quote(t),
        Some(Tok::Name(s)) => repr_str(s),
        Some(Tok::Op(s)) => repr_str(s),
    }
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

pub fn lex(s: &str, line: Option<usize>) -> PResult<Vec<Tok>> {
    let s = s.trim_end();
    let chars: Vec<char> = s.chars().collect();
    let (mut toks, mut pos) = (Vec::new(), 0);
    while pos < chars.len() {
        let mut p = pos;
        while p < chars.len() && chars[p].is_whitespace() {
            p += 1;
        }
        let c = chars[p];
        if c == HOLE {
            // a literal the module parser took out of the line (`lift_texts`)
            let start = p + 1;
            p = start;
            while p < chars.len() && chars[p] != HOLE {
                p += 1;
            }
            let i: usize = chars[start..p].iter().collect::<String>().parse().unwrap_or(usize::MAX);
            let t = LIFTED.with(|l| l.borrow().get(i).cloned());
            toks.push(Tok::Text(t.ok_or_else(|| ParseError::at("bad text literal", line))?));
            pos = p + 1;
            continue;
        }
        if c == '"' {
            let (t, end) = read_quoted(&chars, p).ok_or_else(|| ParseError::at("unterminated text literal", line))?;
            toks.push(Tok::Text(t));
            pos = end;
            continue;
        }
        if c.is_ascii_digit() {
            let start = p;
            while p < chars.len() && chars[p].is_ascii_digit() {
                p += 1;
            }
            let text: String = chars[start..p].iter().collect();
            let n: Int = text
                .parse()
                .ok()
                .filter(|n| in_range(*n))
                .ok_or_else(|| ParseError::at(out_of_range(&text), line))?;
            toks.push(Tok::Int(n));
        } else if is_name_start(c) {
            let start = p;
            while p < chars.len() && is_name_char(chars[p]) {
                p += 1;
            }
            if p < chars.len() && chars[p] == '\'' {
                p += 1;
            }
            toks.push(Tok::Name(chars[start..p].iter().collect()));
        } else {
            let two: String = chars[p..(p + 2).min(chars.len())].iter().collect();
            if ["!=", ">=", "<=", "->"].contains(&two.as_str()) {
                toks.push(Tok::Op(op(&two)));
                p += 2;
            } else if "=<>+-*/()".contains(c) {
                toks.push(Tok::Op(op(&c.to_string())));
                p += 1;
            } else {
                let rest: String = chars[pos..].iter().collect();
                let first: String = rest.trim().chars().take(1).collect();
                return Err(ParseError::at(
                    format!("unexpected character {} in {}", repr_str(&first), repr_str(s.trim())),
                    line,
                ));
            }
        }
        pos = p;
    }
    Ok(toks)
}

/// Marks a lifted text literal inside a line: HOLE index HOLE (a character no source line contains).
const HOLE: char = '\u{1}';

thread_local! {
    /// The text literals lift_texts took out of the lines of the module being parsed, by index.
    static LIFTED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `"…"` starting at chars[p]: its content (escapes `\"` and `\\`) and the index after the closing quote.
fn read_quoted(chars: &[char], p: usize) -> Option<(String, usize)> {
    let (mut out, mut i) = (String::new(), p + 1);
    while i < chars.len() {
        match chars[i] {
            '"' => return Some((out, i + 1)),
            '\\' if i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\\') => {
                out.push(chars[i + 1]);
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    None
}

/// A Text value as Kern source: quoted, with `"` and `\` escaped.
pub fn quote(t: &str) -> String {
    format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Take every `"…"` out of a line, leaving a space-free placeholder, so the line-level splits (`else`, `when`, `|`,
/// `->`, whitespace in `eg` lines) never look inside a literal. The lexer and the `eg` parser put them back.
fn lift_texts(line: &str, no: Option<usize>) -> PResult<String> {
    if !line.contains('"') {
        return Ok(line.to_string());
    }
    let chars: Vec<char> = line.chars().collect();
    let (mut out, mut p) = (String::new(), 0);
    while p < chars.len() {
        if chars[p] == '"' {
            let (t, end) = read_quoted(&chars, p).ok_or_else(|| ParseError::at("unterminated text literal", no))?;
            let i = LIFTED.with(|l| {
                let mut l = l.borrow_mut();
                l.push(t);
                l.len() - 1
            });
            out.push(HOLE);
            out.push_str(&i.to_string());
            out.push(HOLE);
            p = end;
        } else {
            out.push(chars[p]);
            p += 1;
        }
    }
    Ok(out)
}

/// A line (or part of one) with its lifted literals written back as source.
pub fn restore(s: &str) -> String {
    if !s.contains(HOLE) {
        return s.to_string();
    }
    let mut out = String::new();
    let mut parts = s.split(HOLE);
    out.push_str(parts.next().unwrap_or(""));
    while let (Some(i), Some(rest)) = (parts.next(), parts.next()) {
        let t = LIFTED.with(|l| i.parse::<usize>().ok().and_then(|i| l.borrow().get(i).cloned())).unwrap_or_default();
        out.push_str(&quote(&t));
        out.push_str(rest);
    }
    out
}

/// A lifted literal standing alone (`eg` arguments and values): its text.
fn lifted(s: &str) -> Option<String> {
    let inner = s.strip_prefix(HOLE)?.strip_suffix(HOLE)?;
    let i: usize = inner.parse().ok()?;
    LIFTED.with(|l| l.borrow().get(i).cloned())
}

// ---------------------------------------------------------------- expressions

struct ExprParser {
    t: Vec<Tok>,
    i: usize,
    line: Option<usize>,
}

impl ExprParser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn peek_name(&self, n: &str) -> bool {
        matches!(self.peek(), Some(Tok::Name(s)) if s == n)
    }
    fn peek_op(&self) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }
    fn take(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn err(&self, msg: String) -> ParseError {
        ParseError::at(msg, self.line)
    }
    fn expect_end(&self) -> PResult<()> {
        if self.i != self.t.len() {
            return Err(self.err(format!("unexpected {}", tok_repr(self.peek()))));
        }
        Ok(())
    }
    fn expr(&mut self) -> PResult<Expr> {
        let left = self.or_()?;
        if self.peek_name("when") {
            self.take();
            return Ok(Expr::Bin("when", Box::new(left), Box::new(self.or_()?)));
        }
        Ok(left)
    }
    fn or_(&mut self) -> PResult<Expr> {
        let mut left = self.and_()?;
        while self.peek_name("or") {
            self.take();
            left = Expr::Bin("or", Box::new(left), Box::new(self.and_()?));
        }
        Ok(left)
    }
    fn and_(&mut self) -> PResult<Expr> {
        let mut left = self.cmp()?;
        while self.peek_name("and") {
            self.take();
            left = Expr::Bin("and", Box::new(left), Box::new(self.cmp()?));
        }
        Ok(left)
    }
    fn cmp(&mut self) -> PResult<Expr> {
        let left = self.sum()?;
        if let Some(o) = self.peek_op() {
            if CMP.contains(&o) {
                self.take();
                return Ok(Expr::Bin(o, Box::new(left), Box::new(self.sum()?)));
            }
        }
        Ok(left)
    }
    fn sum(&mut self) -> PResult<Expr> {
        let mut left = self.term()?;
        while let Some(o @ ("+" | "-")) = self.peek_op() {
            self.take();
            left = Expr::Bin(o, Box::new(left), Box::new(self.term()?));
        }
        Ok(left)
    }
    fn term(&mut self) -> PResult<Expr> {
        let mut left = self.app()?;
        while let Some(o @ ("*" | "/")) = self.peek_op() {
            self.take();
            left = Expr::Bin(o, Box::new(left), Box::new(self.app()?));
        }
        Ok(left)
    }
    /// `sum j from A to B of E [where C]`, at `sum` followed by a name and `from`.
    fn is_sum(&self) -> bool {
        self.peek_name("sum")
            && matches!(self.t.get(self.i + 1), Some(Tok::Name(_)))
            && matches!(self.t.get(self.i + 2), Some(Tok::Name(f)) if f == "from")
    }
    fn word(&mut self, w: &str) -> PResult<()> {
        if !self.peek_name(w) {
            return Err(self.err(format!("a sum is `sum j from A to B of E [where C]`: expected `{}`", w)));
        }
        self.take();
        Ok(())
    }
    fn sum_expr(&mut self) -> PResult<Expr> {
        self.take();
        let Some(Tok::Name(j)) = self.take() else { unreachable!() };
        if !j.chars().next().map_or(false, |c| c.is_ascii_lowercase()) || j.contains('.') || j.ends_with('\'') {
            return Err(self.err(format!("the sum's variable must be a plain lower-case name, got {}", repr_str(&j))));
        }
        self.word("from")?;
        let lo = self.atom()?;
        if !self.peek_name("to") {
            return Err(self.err("`sum j from A to B of E`: each bound is one atom, so parenthesise a compound one: `to (lo + n)`".into()));
        }
        self.word("to")?;
        let hi = self.atom()?;
        if !self.peek_name("of") {
            return Err(self.err("`sum j from A to B of E`: each bound is one atom, so parenthesise a compound one: `to (lo + n)`".into()));
        }
        self.word("of")?;
        let body = self.sum()?;
        let cond = if self.peek_name("where") {
            self.take();
            Some(Box::new(self.or_()?))
        } else {
            None
        };
        Ok(Expr::Sum(j, Box::new(lo), Box::new(hi), Box::new(body), cond))
    }
    fn app(&mut self) -> PResult<Expr> {
        if self.is_sum() {
            return self.sum_expr();
        }
        if let Some(Tok::Name(val)) = self.peek() {
            if !KEYWORDS.contains(&val.as_str()) {
                let val = val.clone();
                self.take();
                let mut args = Vec::new();
                while self.starts_atom() {
                    args.push(self.atom()?);
                }
                let (name, primed) = split_prime(&val);
                if !args.is_empty() {
                    return Ok(Expr::App(name, primed, args));
                }
                return Ok(Expr::Name(name, primed));
            }
        }
        self.atom()
    }
    fn starts_atom(&self) -> bool {
        match self.peek() {
            Some(Tok::Int(_)) | Some(Tok::Text(_)) => true,
            Some(Tok::Name(v)) => !KEYWORDS.contains(&v.as_str()),
            Some(Tok::Op("(")) => true,
            _ => false,
        }
    }
    fn atom(&mut self) -> PResult<Expr> {
        if self.is_sum() {
            return Err(self.err("a sum as an argument needs parentheses: f (sum j from A to B of E)".into()));
        }
        let tok = self.take();
        match tok {
            Some(Tok::Int(n)) => Ok(Expr::Int(n)),
            Some(Tok::Text(t)) => Ok(Expr::Text(t)),
            Some(Tok::Name(v)) => {
                let (name, primed) = split_prime(&v);
                Ok(Expr::Name(name, primed))
            }
            Some(Tok::Op("(")) => {
                let e = self.expr()?;
                if self.take() != Some(Tok::Op(")")) {
                    return Err(self.err("missing ')'".into()));
                }
                Ok(e)
            }
            other => Err(self.err(format!("expected a value, got {}", tok_repr(other.as_ref())))),
        }
    }
}

fn split_prime(v: &str) -> (String, bool) {
    match v.strip_suffix('\'') {
        Some(n) => (n.to_string(), true),
        None => (v.to_string(), false),
    }
}

pub fn parse_expr(src: &str, line: Option<usize>) -> PResult<Expr> {
    let mut p = ExprParser { t: lex(src, line)?, i: 0, line };
    let e = p.expr()?;
    p.expect_end()?;
    Ok(e)
}

// ---------------------------------------------------------------- clauses

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

const ATOM: &str = r"(\([^()]*\)|\S+)";

fn for_suffix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, &format!(r"^(?s:(.*)) for ([a-z]\w*) from {ATOM} to {ATOM}$"))
}
fn for_loop() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, &format!(r"^for ([a-z]\w*) from {ATOM} to {ATOM}$"))
}

fn loose_for() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?:^|\s)for\s+[a-z]\w*\s+from\s+\S.*\s+to\s+\S")
}

fn compound_bound(line: Option<usize>) -> ParseError {
    ParseError::at("`for j from A to B`: each bound is one atom, so parenthesise a compound one: `to (lo + n)`", line)
}

/// An ensure / loop-invariant clause: `E [when C] [for j from A to B]`.
pub fn parse_clause(src: &str, line: Option<usize>) -> PResult<Expr> {
    if for_suffix().captures(src.trim()).is_none() && loose_for().is_match(src.trim()) {
        return Err(compound_bound(line));
    }
    if let Some(m) = for_suffix().captures(src.trim()) {
        return Ok(Expr::All(
            m[2].to_string(),
            Box::new(parse_expr(&m[3], line)?),
            Box::new(parse_expr(&m[4], line)?),
            Box::new(parse_expr(&m[1], line)?),
        ));
    }
    parse_expr(src, line)
}

/// `emit Ctor ARGS [when COND]`.
pub fn parse_emit(src: &str, line: Option<usize>) -> PResult<Emit> {
    if let Some((ev, cond)) = src.split_once(" when ") {
        let (ctor, args) = parse_ctor(ev, line, "emit")?;
        return Ok(Emit::Ev(ctor, args, Some(parse_expr(cond, line)?)));
    }
    let (ctor, args) = parse_ctor(src, line, "emit")?;
    Ok(Emit::Ev(ctor, args, None))
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().map_or(false, |c| c.is_uppercase())
}

pub fn parse_ctor(src: &str, line: Option<usize>, what: &str) -> PResult<(String, Vec<Expr>)> {
    let (ctor, args) = match parse_expr(src, line)? {
        Expr::Name(n, false) => (n, vec![]),
        Expr::App(n, false, args) => (n, args),
        _ => return Err(ParseError::at(format!("{} must name a constructor", what), line)),
    };
    if !starts_upper(&ctor) {
        return Err(ParseError::at(format!("{} must be Capitalised", repr_str(&ctor)), line));
    }
    Ok((ctor, args))
}

pub fn parse_need(src: &str, line: Option<usize>) -> PResult<(Expr, String, Vec<Expr>)> {
    let Some((cond, fail)) = src.rsplit_once(" else ") else {
        return Err(ParseError::at("need must say what it fails with: need COND else Ctor", line));
    };
    if for_suffix().is_match(cond.trim()) {
        return Err(ParseError::at(
            "`for j from A to B` is only allowed at the end of an `ensure` or a loop `invariant`",
            line,
        ));
    }
    let (ctor, args) = match parse_expr(fail, line)? {
        Expr::Name(n, _) => (n, vec![]),
        Expr::App(n, _, args) => (n, args),
        _ => return Err(ParseError::at("else must name a failure constructor", line)),
    };
    if !starts_upper(&ctor) {
        return Err(ParseError::at(format!("failure {} must be Capitalised", repr_str(&ctor)), line));
    }
    Ok((parse_expr(cond, line)?, ctor, args))
}

// ---------------------------------------------------------------- worlds and examples

/// Python's str.splitlines (line boundaries \n \r \r\n \v \f \x1c-\x1e \x85    ).
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut it) = (0, text.char_indices().peekable());
    while let Some((i, c)) = it.next() {
        let end = match c {
            '\r' => {
                if let Some((_, '\n')) = it.peek() {
                    it.next();
                    i + 2
                } else {
                    i + 1
                }
            }
            '\n' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                i + c.len_utf8()
            }
            _ => continue,
        };
        out.push(&text[start..i]);
        start = end;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn world_item() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^([A-Z]\w*):(\d+)(?:@(\w+))?$")
}
fn now_item() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^now:(\d+)$")
}
fn map_item() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^([a-z]\w*)\.([A-Z]\w*|-?\d+):(-?\d+)$")
}

/// Kern's Int is i64 at run time (the host's store, `kern build` programs), so a literal outside it is refused
/// where it is written, not wrapped or widened later (SHORTCOMINGS S20).
pub fn in_range(n: Int) -> bool {
    (i64::MIN as Int..=i64::MAX as Int).contains(&n)
}

fn out_of_range(text: &str) -> String {
    format!("integer {} is outside Int's range ({} to {})", text, i64::MIN, i64::MAX)
}

fn int(s: &str, line: Option<usize>) -> PResult<Int> {
    let n: Int = s.parse().map_err(|_| ParseError::at(format!("bad integer {}", repr_str(s)), line))?;
    if !in_range(n) {
        return Err(ParseError::at(out_of_range(s), line));
    }
    Ok(n)
}

pub type Maps = IndexMap<String, IndexMap<Key, Int>>;
/// Text state entries: state -> {key: text}
pub type Texts = IndexMap<String, IndexMap<Key, String>>;

fn text_item() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^([a-z]\w*)\.([A-Z]\w*|-?\d+):(\x01\d+\x01)$")
}

/// Take `state.key:"text"` items out of an `eg` world or after-state: (rest, {state: {key: text}}).
fn split_texts(src: &str, line: Option<usize>) -> PResult<(String, Texts)> {
    let (mut items, mut texts) = (Vec::new(), Texts::new());
    for item in src.split_whitespace() {
        if let Some(m) = text_item().captures(item) {
            let k = &m[2];
            let key = if k.chars().next().unwrap().is_alphabetic() { Key::Str(k.into()) } else { Key::Int(int(k, line)?) };
            texts.entry(m[1].to_string()).or_default().insert(key, lifted(&m[3]).unwrap_or_default());
        } else if item.contains(HOLE) {
            return Err(ParseError::at(format!("bad world item {} (a text entry is state.key:\"…\")", restore(item)), line));
        } else {
            items.push(item);
        }
    }
    Ok((items.join(" "), texts))
}

/// Take `state.key:value` items out of a world: (rest, {state: {key: value}}).
pub fn split_maps(src: &str) -> PResult<(String, Maps)> {
    let (mut items, mut maps) = (Vec::new(), Maps::new());
    for item in src.split_whitespace() {
        if let Some(m) = map_item().captures(item) {
            let k = &m[2];
            let key = if k.chars().next().unwrap().is_alphabetic() { Key::Str(k.into()) } else { Key::Int(int(k, None)?) };
            maps.entry(m[1].to_string()).or_default().insert(key, int(&m[3], None)?);
        } else {
            items.push(item);
        }
    }
    Ok((items.join(" "), maps))
}

/// Take an optional `now:N` item out of a world: (rest, N).
pub fn split_now(src: &str) -> PResult<(String, Int)> {
    let (mut items, mut now) = (Vec::new(), 0);
    for item in src.split_whitespace() {
        if let Some(m) = now_item().captures(item) {
            now = int(&m[1], None)?;
        } else {
            items.push(item);
        }
    }
    Ok((items.join(" "), now))
}

/// `A:500@u1 ...` -> account -> (balance, owner).
pub fn parse_world(src: &str, line: Option<usize>) -> PResult<IndexMap<String, (Int, String)>> {
    let mut w = IndexMap::new();
    for item in src.split_whitespace() {
        match world_item().captures(item) {
            Some(m) if m.get(3).is_some() => {
                w.insert(m[1].to_string(), (int(&m[2], line)?, m[3].to_string()));
            }
            _ => return Err(bad_world(item, line)),
        }
    }
    Ok(w)
}

/// `A:500 ...` (owners allowed and ignored) -> account -> balance.
pub fn parse_balances(src: &str, line: Option<usize>) -> PResult<IndexMap<String, Int>> {
    let mut w = IndexMap::new();
    for item in src.split_whitespace() {
        match world_item().captures(item) {
            Some(m) => {
                w.insert(m[1].to_string(), int(&m[2], line)?);
            }
            None => return Err(bad_world(item, line)),
        }
    }
    Ok(w)
}

fn bad_world(item: &str, line: Option<usize>) -> ParseError {
    ParseError::at(format!("bad world item {} (want A:500@u1)", repr_str(item)), line)
}

pub fn parse_example(src: &str, line: Option<usize>) -> PResult<Example> {
    let usage = || ParseError::at("eg WORLD | call args -> outcome [| AFTER [| EVENTS]]", line);
    // literals out first (a no-op for a line parse_module already lifted), so `|`, `->` and spaces inside one
    // don't split it; `src` keeps the source text
    let mark = LIFTED.with(|l| l.borrow().len());
    let lifted_src = lift_texts(src, line)?;
    let src_text = restore(&lifted_src);
    let parts: Vec<&str> = lifted_src.split('|').map(str::trim).collect();
    if !(2..=4).contains(&parts.len()) || !parts[1].contains("->") {
        return Err(usage());
    }
    // split_now / split_maps are line-free (the host parses worlds too): put the eg's line on their errors
    let at = |e: ParseError| ParseError::at(e.0, line);
    let (world_src, texts) = split_texts(parts[0], line)?;
    let (world_src, now) = split_now(&world_src).map_err(at)?;
    let (world_src, maps) = split_maps(&world_src).map_err(at)?;
    let world = parse_world(&world_src, line)?;
    let sides: Vec<&str> = parts[1].split("->").map(str::trim).collect();
    if sides.len() != 2 {
        return Err(usage());
    }
    let call: Vec<&str> = sides[0].split_whitespace().collect();
    if call.is_empty() {
        return Err(usage());
    }
    let mut args = Vec::new();
    for a in &call[1..] {
        if let Some(t) = lifted(a) {
            args.push(Key::Str(t));
            continue;
        }
        let digits = a.trim_start_matches('-');
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            let n: Int = a.parse().map_err(|_| usage())?;
            if !in_range(n) {
                return Err(ParseError::at(out_of_range(a), line));
            }
            args.push(Key::Int(n));
        } else {
            args.push(Key::Str(a.to_string()));
        }
    }
    let out: Vec<&str> = sides[1].split_whitespace().collect();
    let outcome = if out == ["ok"] {
        Outcome::Ok
    } else {
        let Some((ctor, rest)) = out.split_first() else { return Err(usage()) };
        if rest.iter().any(|x| x.contains(HOLE)) {
            return Err(ParseError::at("a refusal's payload is Int only (no Text in an `eg` outcome)", line));
        }
        let vals: Result<Vec<Int>, _> = rest.iter().map(|x| x.parse::<Int>()).collect();
        if let Some(x) = vals.as_ref().ok().and_then(|v| v.iter().zip(rest).find(|(n, _)| !in_range(**n))) {
            return Err(ParseError::at(out_of_range(x.1), line));
        }
        Outcome::Fail(ctor.to_string(), vals.map_err(|_| usage())?)
    };
    let (after_src, after_texts) = if parts.len() >= 3 { split_texts(parts[2], line)? } else { (String::new(), Texts::new()) };
    let (after_src, after_maps) = if parts.len() >= 3 { split_maps(&after_src).map_err(at)? } else { (String::new(), Maps::new()) };
    let events = if parts.len() == 4 { Some(parse_eg_events(parts[3], line)?) } else { None };
    let after = parse_balances(&after_src, line)?;
    // what this call lifted is copied out by now (a module's own lines stay lifted for its body parse)
    LIFTED.with(|l| l.borrow_mut().truncate(mark));
    Ok(Example {
        world,
        call: (call[0].to_string(), args),
        outcome,
        after,
        src: src_text,
        now,
        maps,
        after_maps,
        texts,
        after_texts,
        events,
    })
}

/// An `eg` line's events section: `Ev args, [channel] Ev args, …` (empty: nothing on audit).
fn parse_eg_events(src: &str, line: Option<usize>) -> PResult<Vec<(String, String, Vec<Key>)>> {
    let mut out = vec![];
    for ev in src.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let mut words: Vec<&str> = ev.split_whitespace().collect();
        let channel = if words.first().is_some_and(|w| w.starts_with(|c: char| c.is_ascii_lowercase())) {
            words.remove(0).to_string()
        } else {
            "audit".to_string()
        };
        let Some((ctor, args)) = words.split_first() else {
            return Err(ParseError::at("an `eg` event is `[channel] Ctor args`", line));
        };
        if !starts_upper(ctor) {
            return Err(ParseError::at(format!("an `eg` event names a constructor, got {}", repr_str(ctor)), line));
        }
        let mut keys = vec![];
        for a in args {
            if let Some(t) = lifted(a) {
                keys.push(Key::Str(t));
            } else if let Ok(n) = a.parse::<Int>() {
                if !in_range(n) {
                    return Err(ParseError::at(out_of_range(a), line));
                }
                keys.push(Key::Int(n));
            } else {
                keys.push(Key::Str(a.to_string()));
            }
        }
        out.push((channel, ctor.to_string(), keys));
    }
    Ok(out)
}

/// A call and a world written as an `eg` line would have them (`kern run`, MCP `kern_run`), for input with Text in
/// it: quote-aware, so `name_it 1 "a b"` is two arguments.
pub fn parse_call(world: &str, call: &str) -> PResult<Example> {
    parse_example(&format!("{} | {} -> ok", world, call), None)
}

// ---------------------------------------------------------------- modules

fn header() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^([a-z]\w*)((?:\s+\w+:\w+)*)\s*(?:->\s*(\w+))?\s*$")
}
fn state_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^state ([a-z]\w*) (Int|Account)(?: -> (Int|Text))?$")
}
fn channel_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^channel ([a-z]\w*)$")
}
fn handler_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^handler ([a-z]\w*(?:\.[a-z]\w*)+)((?:\s+\w+:\w+)*)\s*(?:->\s*(\w+))?\s*$")
}
fn const_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^const ([a-z]\w*) (\d+)$")
}
fn record_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^record ([A-Z]\w*)((?:\s+[a-z]\w*:\w+)+)\s*$")
}
fn invariant_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"^invariant ([a-z]\w*):(Int|Account) (.+)$")
}

fn params_of(src: &str) -> Vec<(String, String)> {
    src.split_whitespace()
        .map(|p| {
            let (a, b) = p.split_once(':').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect()
}

fn src_lines(header: &str) -> IndexMap<String, Vec<String>> {
    let mut m = IndexMap::new();
    m.insert("header".to_string(), vec![header.to_string()]);
    m.insert("spec".to_string(), vec![]);
    m.insert("body".to_string(), vec![]);
    m
}

/// Python's str.partition(" ") on a stripped line.
fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str) {
    s.split_once(sep).unwrap_or((s, ""))
}

/// `expand = false` leaves `need as` / `emit as` unresolved (the package loader resolves them over all files).
pub fn parse_module(text: &str, expand: bool) -> PResult<Module> {
    let mut defs = Module::default();
    let mut cur: Option<(bool, String)> = None; // (is_handler, name)
    let mut in_body = false;
    let lines = splitlines(text);

    fn cur_def<'m>(defs: &'m mut Module, cur: &Option<(bool, String)>) -> Option<&'m mut Def> {
        match cur {
            Some((true, n)) => defs.handlers.get_mut(n),
            Some((false, n)) => defs.defs.get_mut(n),
            None => None,
        }
    }
    fn finish_cur(defs: &mut Module, cur: &Option<(bool, String)>) -> PResult<()> {
        if let Some(d) = cur_def(defs, cur) {
            finish(d)?;
        }
        Ok(())
    }

    LIFTED.with(|l| l.borrow_mut().clear());
    for (idx, raw) in lines.iter().enumerate() {
        let no = Some(idx + 1);
        if raw.trim().is_empty() {
            continue;
        }
        let lifted_raw = lift_texts(raw, no)?;
        let raw = lifted_raw.as_str();
        let line = raw.trim();
        if !raw.chars().next().unwrap().is_whitespace() {
            if line.starts_with("import ") {
                return Err(ParseError::at(
                    "`import` needs the package loader (python -m kern check FILE loads it)",
                    no,
                ));
            }
            if line.starts_with("handler ") {
                let Some(m) = handler_re().captures(line) else {
                    return Err(ParseError::at("want `handler NAME.op p:Type ... -> Int`", no));
                };
                finish_cur(&mut defs, &cur)?;
                let mut d = Def::new(&m[1], params_of(&m[2]), m.get(3).map(|x| x.as_str().to_string()));
                d.src_lines = src_lines(line);
                d.assumed = true;
                cur = Some((true, d.name.clone()));
                defs.handlers.insert(d.name.clone(), d);
                in_body = false;
                continue;
            }
            // `record Name …` (a definition may itself be named `record`: `record x:Int`)
            if line.starts_with("record ") && line["record ".len()..].starts_with(|c: char| c.is_uppercase()) {
                let Some(m) = record_re().captures(line) else {
                    return Err(ParseError::at("want `record Name field:Type ...` (Int or Account fields)", no));
                };
                if defs.records.contains_key(&m[1]) {
                    return Err(ParseError::at(format!("record {} is declared twice", &m[1]), no));
                }
                defs.records.insert(m[1].to_string(), params_of(&m[2]));
                finish_cur(&mut defs, &cur)?;
                cur = None;
                in_body = false;
                continue;
            }
            // `const NAME N` (a definition may itself be named `const`: `const x:Int`)
            if line.starts_with("const ") && !line.contains(':') {
                let Some(m) = const_re().captures(line) else {
                    return Err(ParseError::at("want `const NAME N` (a lower-case name, a non-negative integer)", no));
                };
                if RESERVED.contains(&&m[1]) {
                    return Err(ParseError::at(format!("`{}` is reserved and can't name a constant", &m[1]), no));
                }
                if defs.consts.contains_key(&m[1]) {
                    return Err(ParseError::at(format!("constant {} is declared twice", &m[1]), no));
                }
                defs.consts.insert(m[1].to_string(), int(&m[2], no)?);
                finish_cur(&mut defs, &cur)?;
                cur = None;
                in_body = false;
                continue;
            }
            if line.starts_with("state ") || line.starts_with("invariant ") || line.starts_with("channel ") {
                if let Some(ch) = channel_re().captures(line) {
                    if &ch[1] == "audit" || &ch[1] == "as" {
                        return Err(ParseError::at(format!("`{}` can't be declared as a channel", &ch[1]), no));
                    }
                    defs.channels.push(ch[1].to_string());
                } else if let Some(m) = state_re().captures(line) {
                    defs.states.insert(m[1].to_string(), m[2].to_string());
                    if m.get(3).is_some_and(|v| v.as_str() == "Text") {
                        defs.text_states.push(m[1].to_string());
                    }
                } else if let Some(inv) = invariant_re().captures(line) {
                    let e = parse_expr(&inv[3], no)?;
                    defs.invariants.push((inv[1].to_string(), inv[2].to_string(), e));
                } else {
                    return Err(ParseError::at(
                        "want `state NAME Int|Account [-> Text]`, `invariant x:Type COND` or `channel NAME`",
                        no,
                    ));
                }
                finish_cur(&mut defs, &cur)?;
                cur = None;
                in_body = false;
                continue;
            }
            if line == "body" {
                match &cur {
                    None => return Err(ParseError::at("body without a definition", no)),
                    Some((true, n)) => {
                        return Err(ParseError::at(
                            format!("handler {} has an assumed spec and no body", n),
                            no,
                        ))
                    }
                    _ => {}
                }
                in_body = true;
                continue;
            }
            let Some(m) = header().captures(line) else {
                return Err(ParseError::at(format!("bad definition header {}", repr_str(line)), no));
            };
            let params = params_of(&m[2]);
            finish_cur(&mut defs, &cur)?;
            let mut d = Def::new(&m[1], params, m.get(3).map(|x| x.as_str().to_string()));
            d.src_lines = src_lines(line);
            cur = Some((false, d.name.clone()));
            defs.defs.insert(d.name.clone(), d);
            in_body = false;
            continue;
        }
        let Some(d) = cur_def(&mut defs, &cur) else {
            return Err(ParseError::at("indented line outside a definition", no));
        };
        let (word, rest) = partition(line, " ");
        if in_body {
            let indent = raw.chars().count() - raw.trim_start().chars().count();
            d.raw.push((indent, line.to_string(), idx + 1));
            continue;
        }
        d.src_lines.get_mut("spec").unwrap().push(restore(line));
        if word == "need" && rest.starts_with("as ") {
            let (callee, args) = ref_(&rest[3..], no)?;
            d.needs.push(Need::As(callee, args, idx + 1));
        } else if word == "emit" && rest.starts_with("as ") {
            let (r, cond) = partition(&rest[3..], " when ");
            let (callee, args) = ref_(r, no)?;
            let cond = if cond.is_empty() { None } else { Some(parse_expr(cond, no)?) };
            d.emits.push(Emit::As(callee, args, idx + 1, cond));
        } else if word == "need" {
            let (c, ctor, args) = parse_need(rest, no)?;
            d.needs.push(Need::Need(c, ctor, args));
        } else if word == "keep" {
            d.keeps.push(parse_expr(rest, no)?);
        } else if word == "ensure" {
            d.ensures.push(parse_clause(rest, no)?);
        } else if word == "emit" {
            let (first, after) = partition(rest, " ");
            let mut rest = rest;
            if first.chars().next().map_or(false, |c| c.is_lowercase()) && !after.is_empty() {
                d.emit_channel.insert(d.emits.len(), first.to_string());
                rest = after;
            }
            d.emits.push(parse_emit(rest, no)?);
        } else if word == "eg" {
            d.examples.push(parse_example(rest, no)?);
        } else if word == "ok" {
            if rest.split_whitespace().collect::<Vec<_>>() != ["otherwise"] {
                return Err(ParseError::at(
                    "the clause is `ok otherwise` (succeeds whenever the needs hold)",
                    no,
                ));
            }
            if d.complete {
                return Err(ParseError::at("`ok otherwise` appears twice", no));
            }
            d.complete = true;
        } else {
            return Err(ParseError::at(
                format!("unknown clause {} (need/keep/ensure/emit/ok/eg, then body)", repr_str(word)),
                no,
            ));
        }
    }
    finish_cur(&mut defs, &cur)?;
    if expand {
        let consts = defs.consts.clone();
        resolve_consts(&mut defs, &consts, &IndexMap::new())?;
        flatten_records(&mut defs)?;
        expand_refs(&mut defs)?;
    }
    Ok(defs)
}

fn ref_(src: &str, line: Option<usize>) -> PResult<(String, Vec<Expr>)> {
    match parse_expr(src, line)? {
        Expr::App(n, false, args) => Ok((n, args)),
        Expr::Name(n, false) => Ok((n, vec![])),
        _ => Err(ParseError::at("`as` must name a definition and its arguments", line)),
    }
}

/// `e` with unprimed names replaced by `m`.
pub fn subst(e: &Expr, m: &IndexMap<String, Expr>) -> Expr {
    match e {
        Expr::Name(n, false) => m.get(n).cloned().unwrap_or_else(|| e.clone()),
        Expr::Bin(op, l, r) => Expr::Bin(op, Box::new(subst(l, m)), Box::new(subst(r, m))),
        Expr::App(h, p, args) => Expr::App(h.clone(), *p, args.iter().map(|a| subst(a, m)).collect()),
        Expr::Sum(j, a, b, x, c) => {
            let mut inner = m.clone();
            inner.shift_remove(j);
            Expr::Sum(
                j.clone(),
                Box::new(subst(a, m)),
                Box::new(subst(b, m)),
                Box::new(subst(x, &inner)),
                c.as_ref().map(|c| Box::new(subst(c, &inner))),
            )
        }
        _ => e.clone(),
    }
}

/// Words that can't name a constant: they are keywords, or a constant would hide them.
const RESERVED: [&str; 32] = [
    "and", "or", "else", "when", "as", "result", "body", "need", "keep", "ensure", "emit", "eg", "ok", "if", "for",
    "from", "to", "distinct", "sum", "of", "where", "const", "import", "state", "record", "channel", "invariant",
    "handler", "ledger", "clock", "audit", "length",
];

/// Substitutes named constants into a definition, collecting what it used.
struct Consts<'a> {
    visible: &'a IndexMap<String, Int>,
    /// constant -> the module that declares it, for constants this module doesn't import
    hidden: &'a IndexMap<String, String>,
    who: String,
}

impl Consts<'_> {
    fn is_const(&self, n: &str) -> bool {
        self.visible.contains_key(n) || self.hidden.contains_key(n)
    }
    fn err(&self, msg: String) -> ParseError {
        ParseError(format!("{}: {}", self.who, msg))
    }
    fn binder(&self, n: &str, what: &str) -> PResult<()> {
        if self.is_const(n) {
            return Err(self.err(format!("{} {} has the name of a constant; rename it", what, n)));
        }
        Ok(())
    }
    fn expr(&self, e: &Expr, used: &mut Vec<(String, Int)>) -> PResult<Expr> {
        Ok(match e {
            Expr::Int(_) | Expr::Text(_) => e.clone(),
            Expr::Name(n, primed) | Expr::App(n, primed, _) if self.is_const(n) => {
                if let Some(m) = self.hidden.get(n) {
                    return Err(self.err(format!("uses const {}, which {} declares: add `import {}`", n, m, m)));
                }
                if *primed {
                    return Err(self.err(format!("`{}'`: a constant has no after-state", n)));
                }
                if let Expr::App(..) = e {
                    return Err(self.err(format!(
                        "`{}`: constant {} takes no arguments (if this is a range bound, parenthesise it: `to (lo + {})`)",
                        code(e),
                        n,
                        n
                    )));
                }
                let v = self.visible[n];
                if !used.iter().any(|(u, _)| u == n) {
                    used.push((n.clone(), v));
                }
                Expr::Int(v)
            }
            Expr::Name(..) => e.clone(),
            Expr::App(h, p, args) => {
                Expr::App(h.clone(), *p, args.iter().map(|a| self.expr(a, used)).collect::<PResult<_>>()?)
            }
            Expr::Bin(op, l, r) => Expr::Bin(op, Box::new(self.expr(l, used)?), Box::new(self.expr(r, used)?)),
            Expr::All(j, a, b, x) => {
                self.binder(j, "the range variable")?;
                Expr::All(
                    j.clone(),
                    Box::new(self.expr(a, used)?),
                    Box::new(self.expr(b, used)?),
                    Box::new(self.expr(x, used)?),
                )
            }
            Expr::Sum(j, a, b, x, c) => {
                self.binder(j, "the sum's variable")?;
                Expr::Sum(
                    j.clone(),
                    Box::new(self.expr(a, used)?),
                    Box::new(self.expr(b, used)?),
                    Box::new(self.expr(x, used)?),
                    c.as_ref().map(|c| self.expr(c, used).map(Box::new)).transpose()?,
                )
            }
        })
    }
    fn exprs(&self, es: &[Expr], used: &mut Vec<(String, Int)>) -> PResult<Vec<Expr>> {
        es.iter().map(|e| self.expr(e, used)).collect()
    }
    fn stmts(&self, ss: &[Stmt], used: &mut Vec<(String, Int)>) -> PResult<Vec<Stmt>> {
        ss.iter()
            .map(|s| {
                Ok(match s {
                    Stmt::Let(x, e) => {
                        self.binder(x, "the `let`")?;
                        Stmt::Let(x.clone(), self.expr(e, used)?)
                    }
                    Stmt::Need(c, ctor, args) => Stmt::Need(self.expr(c, used)?, ctor.clone(), self.exprs(args, used)?),
                    Stmt::Do(e) => Stmt::Do(self.expr(e, used)?),
                    Stmt::If(c, t, o) => Stmt::If(self.expr(c, used)?, self.stmts(t, used)?, self.stmts(o, used)?),
                    Stmt::For(k, lo, hi, invs, body) => {
                        self.binder(k, "the loop variable")?;
                        Stmt::For(
                            k.clone(),
                            self.expr(lo, used)?,
                            self.expr(hi, used)?,
                            self.exprs(invs, used)?,
                            self.stmts(body, used)?,
                        )
                    }
                    Stmt::Inv(e) => Stmt::Inv(self.expr(e, used)?),
                })
            })
            .collect()
    }
    fn def(&self, d: &mut Def) -> PResult<()> {
        for (p, _) in &d.params {
            self.binder(p, "the parameter")?;
        }
        let mut used = vec![];
        let mut needs = vec![];
        for n in &d.needs {
            needs.push(match n {
                Need::Need(c, ctor, args) => Need::Need(self.expr(c, &mut used)?, ctor.clone(), self.exprs(args, &mut used)?),
                Need::As(callee, args, no) => Need::As(callee.clone(), self.exprs(args, &mut used)?, *no),
            });
        }
        d.needs = needs;
        d.keeps = self.exprs(&d.keeps, &mut used)?;
        d.ensures = self.exprs(&d.ensures, &mut used)?;
        let mut emits = vec![];
        for ev in &d.emits {
            let cond = |c: &Option<Expr>, used: &mut Vec<(String, Int)>| -> PResult<Option<Expr>> {
                c.as_ref().map(|c| self.expr(c, used)).transpose()
            };
            emits.push(match ev {
                Emit::Ev(ctor, args, c) => Emit::Ev(ctor.clone(), self.exprs(args, &mut used)?, cond(c, &mut used)?),
                Emit::As(callee, args, no, c) => {
                    Emit::As(callee.clone(), self.exprs(args, &mut used)?, *no, cond(c, &mut used)?)
                }
            });
        }
        d.emits = emits;
        d.spec_consts = used;
        let mut used = vec![];
        d.body = self.stmts(&d.body, &mut used)?;
        d.body_consts = used;
        Ok(())
    }
}

/// Replace every use of a named constant (`const NAME N`) by its value, in each definition, handler and module
/// invariant. `visible`: the constants this module may use; `hidden`: constant -> declaring module, for the
/// package's other constants (using one is an "add import" error). A name that would hide a constant (parameter,
/// `let`, loop or range variable, invariant variable) is an error, as is a constant named like a definition,
/// handler, state or channel.
pub fn resolve_consts(
    defs: &mut Module,
    visible: &IndexMap<String, Int>,
    hidden: &IndexMap<String, String>,
) -> PResult<()> {
    if visible.is_empty() && hidden.is_empty() {
        return Ok(());
    }
    for n in visible.keys().chain(hidden.keys()) {
        let kind = if defs.defs.contains_key(n) {
            "a definition"
        } else if defs.handlers.contains_key(n) {
            "a handler"
        } else if defs.states.contains_key(n) {
            "a state"
        } else if defs.channels.contains(n) {
            "a channel"
        } else if defs.handlers.keys().any(|h| h.split('.').next() == Some(n.as_str())) {
            "a handler's root (as in `rand.below`)"
        } else {
            continue;
        };
        return Err(ParseError(format!("constant {} has the name of {}", n, kind)));
    }
    for d in defs.defs.values_mut().chain(defs.handlers.values_mut()) {
        Consts { visible, hidden, who: d.name.clone() }.def(d)?;
    }
    let mut invs = vec![];
    for (x, t, c) in &defs.invariants {
        let k = Consts { visible, hidden, who: format!("invariant {}:{}", x, t) };
        k.binder(x, "the variable")?;
        invs.push((x.clone(), t.clone(), k.expr(c, &mut vec![])?));
    }
    defs.invariants = invs;
    Ok(())
}

/// Flattens record-typed parameters into their fields (DECISIONS.md, records).
struct Records<'a> {
    records: &'a IndexMap<String, Vec<(String, String)>>,
    /// definition -> its parameters as declared (record types not yet flattened)
    sigs: &'a IndexMap<String, Vec<(String, String)>>,
    /// this definition's record parameters -> their record type
    recs: IndexMap<String, String>,
    who: String,
    /// record types this definition names (header and literals), for the package's import rule
    used: Vec<String>,
}

impl Records<'_> {
    fn err(&self, msg: String) -> ParseError {
        ParseError(format!("{}: {}", self.who, msg))
    }
    fn note(&mut self, r: &str) {
        if !self.used.iter().any(|u| u == r) {
            self.used.push(r.to_string());
        }
    }
    /// The fields of a whole-record argument `arg` for a parameter of record type `want`.
    fn whole(&mut self, arg: &Expr, want: &str, callee: &str) -> PResult<Vec<Expr>> {
        let fields = &self.records[want];
        match arg {
            Expr::Name(q, false) if self.recs.contains_key(q) => {
                if self.recs[q] != want {
                    return Err(self.err(format!("`{}` passes {} ({}) where a {} is wanted", callee, q, self.recs[q], want)));
                }
                Ok(fields.iter().map(|(f, _)| Expr::Name(format!("{}.{}", q, f), false)).collect())
            }
            Expr::App(r, false, args) if r == want => {
                self.note(want);
                if args.len() != fields.len() {
                    return Err(self.err(format!("`{}` has {} field(s), got {}", want, fields.len(), args.len())));
                }
                args.iter().map(|a| self.expr(a)).collect()
            }
            _ => Err(self.err(format!(
                "`{}` takes a {} here: pass a {} parameter, or build one with ({} {})",
                callee,
                want,
                want,
                want,
                fields.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>().join(" ")
            ))),
        }
    }
    /// A call's arguments with each record-typed one replaced by its fields.
    fn args(&mut self, callee: &str, args: &[Expr]) -> PResult<Vec<Expr>> {
        let Some(sig) = self.sigs.get(callee).filter(|s| s.len() == args.len()) else {
            return args.iter().map(|a| self.expr(a)).collect();
        };
        let mut out = vec![];
        for (a, (_, t)) in args.iter().zip(sig.clone()) {
            if self.records.contains_key(&t) {
                out.extend(self.whole(a, &t, callee)?);
            } else {
                out.push(self.expr(a)?);
            }
        }
        Ok(out)
    }
    fn expr(&mut self, e: &Expr) -> PResult<Expr> {
        Ok(match e {
            Expr::Int(_) | Expr::Text(_) => e.clone(),
            Expr::Name(n, _) | Expr::App(n, _, _) if self.records.contains_key(n) => {
                return Err(self.err(format!(
                    "`{}` builds a {} only as the argument of a call that takes one",
                    code(e),
                    n
                )))
            }
            Expr::Name(n, _) if self.recs.contains_key(n) => {
                return Err(self.err(format!(
                    "{} is a {}: read a field ({}.{}) or pass it whole to a definition that takes one",
                    n, self.recs[n], n, self.records[&self.recs[n]][0].0
                )))
            }
            Expr::Name(n, _) => {
                if let Some((q, f)) = n.split_once('.') {
                    if let Some(r) = self.recs.get(q) {
                        if !self.records[r].iter().any(|(x, _)| x == f) {
                            return Err(self.err(format!("{} ({}) has no field {}", q, r, f)));
                        }
                    }
                }
                e.clone()
            }
            Expr::App(h, p, args) => Expr::App(h.clone(), *p, self.args(h, args)?),
            Expr::Bin(op, l, r) => Expr::Bin(op, Box::new(self.expr(l)?), Box::new(self.expr(r)?)),
            Expr::All(j, a, b, x) => {
                Expr::All(j.clone(), Box::new(self.expr(a)?), Box::new(self.expr(b)?), Box::new(self.expr(x)?))
            }
            Expr::Sum(j, a, b, x, c) => Expr::Sum(
                j.clone(),
                Box::new(self.expr(a)?),
                Box::new(self.expr(b)?),
                Box::new(self.expr(x)?),
                c.as_ref().map(|c| self.expr(c).map(Box::new)).transpose()?,
            ),
        })
    }
    fn exprs(&mut self, es: &[Expr]) -> PResult<Vec<Expr>> {
        es.iter().map(|e| self.expr(e)).collect()
    }
    fn stmts(&mut self, ss: &[Stmt]) -> PResult<Vec<Stmt>> {
        let mut out = vec![];
        for s in ss {
            out.push(match s {
                Stmt::Let(x, e) => Stmt::Let(x.clone(), self.expr(e)?),
                Stmt::Need(c, ctor, args) => Stmt::Need(self.expr(c)?, ctor.clone(), self.exprs(args)?),
                Stmt::Do(e) => Stmt::Do(self.expr(e)?),
                Stmt::If(c, t, o) => Stmt::If(self.expr(c)?, self.stmts(t)?, self.stmts(o)?),
                Stmt::For(k, lo, hi, invs, body) => {
                    Stmt::For(k.clone(), self.expr(lo)?, self.expr(hi)?, self.exprs(invs)?, self.stmts(body)?)
                }
                Stmt::Inv(e) => Stmt::Inv(self.expr(e)?),
            });
        }
        Ok(out)
    }
    fn def(&mut self, d: &mut Def) -> PResult<()> {
        let mut needs = vec![];
        for n in &d.needs {
            needs.push(match n {
                Need::Need(c, ctor, args) => Need::Need(self.expr(c)?, ctor.clone(), self.exprs(args)?),
                Need::As(callee, args, no) => Need::As(callee.clone(), self.args(callee, args)?, *no),
            });
        }
        d.needs = needs;
        d.keeps = self.exprs(&d.keeps)?;
        d.ensures = self.exprs(&d.ensures)?;
        let mut emits = vec![];
        for ev in &d.emits {
            emits.push(match ev {
                Emit::Ev(ctor, args, c) => {
                    Emit::Ev(ctor.clone(), self.exprs(args)?, c.as_ref().map(|c| self.expr(c)).transpose()?)
                }
                Emit::As(callee, args, no, c) => Emit::As(
                    callee.clone(),
                    self.args(callee, args)?,
                    *no,
                    c.as_ref().map(|c| self.expr(c)).transpose()?,
                ),
            });
        }
        d.emits = emits;
        d.body = self.stmts(&d.body)?;
        Ok(())
    }
}

/// Flatten record-typed parameters (`p:Point` -> `p.x:Int p.y:Int`) and whole-record arguments (`move p`,
/// `move (Point a b)`) into fields, in every definition. Returns definition -> the record types it names, for the
/// package's import rule. A module without records is left exactly as it was.
pub fn flatten_records(defs: &mut Module) -> PResult<IndexMap<String, Vec<String>>> {
    if defs.records.is_empty() {
        return Ok(IndexMap::new());
    }
    for (r, fields) in &defs.records {
        if ["Int", "Account", "Owner", "Bool", "Event", "Unit", "Text"].contains(&r.as_str()) {
            return Err(ParseError(format!("record {} has the name of a built-in type", r)));
        }
        for (i, (f, t)) in fields.iter().enumerate() {
            if ["get", "set", "emit"].contains(&f.as_str()) {
                return Err(ParseError(format!("record {}: a field can't be named {}", r, f)));
            }
            if fields[..i].iter().any(|(g, _)| g == f) {
                return Err(ParseError(format!("record {}: field {} appears twice", r, f)));
            }
            if t != "Int" && t != "Account" && t != "Text" {
                return Err(ParseError(format!("record {}: field {} must be Int, Account or Text, not {}", r, f, t)));
            }
        }
    }
    for h in defs.handlers.values() {
        if let Some((p, t)) = h.params.iter().find(|(_, t)| defs.records.contains_key(t)) {
            return Err(ParseError(format!("handler {}: parameter {} can't be a record ({})", h.name, p, t)));
        }
    }
    let mut roots: Vec<String> = ["ledger", "clock", "audit"].iter().map(|s| s.to_string()).collect();
    roots.extend(defs.states.keys().cloned());
    roots.extend(defs.channels.iter().cloned());
    roots.extend(defs.handlers.keys().map(|h| h.split('.').next().unwrap().to_string()));
    let sigs: IndexMap<String, Vec<(String, String)>> =
        defs.defs.iter().map(|(n, d)| (n.clone(), d.params.clone())).collect();
    let records = defs.records.clone();
    let mut used = IndexMap::new();
    for d in defs.defs.values_mut() {
        let mut k = Records { records: &records, sigs: &sigs, recs: IndexMap::new(), who: d.name.clone(), used: vec![] };
        let mut params = vec![];
        for (p, t) in &d.params {
            let Some(fields) = records.get(t) else {
                params.push((p.clone(), t.clone()));
                continue;
            };
            if roots.contains(p) {
                return Err(k.err(format!("the record parameter {} has the name of a state, channel or handler", p)));
            }
            k.recs.insert(p.clone(), t.clone());
            k.note(t);
            if !d.records.iter().any(|(r, _)| r == t) {
                d.records.push((t.clone(), fields.clone()));
            }
            params.extend(fields.iter().map(|(f, ft)| (format!("{}.{}", p, f), ft.clone())));
        }
        k.def(d)?;
        d.params = params;
        used.insert(d.name.clone(), k.used);
    }
    Ok(used)
}

/// Replace `need as C args` / `emit as C args` by C's spec needs / emits with C's parameters replaced by args.
/// `ref_origin` marks which need or emit came from which reference (for the page).
pub fn expand_refs(defs: &mut Module) -> PResult<()> {
    let names: Vec<String> = defs.defs.keys().cloned().collect();
    let (mut done, mut busy) = (Vec::<String>::new(), Vec::<String>::new());
    for n in names {
        expand_one(defs, &n, &mut done, &mut busy)?;
    }
    Ok(())
}

fn expand_one(defs: &mut Module, name: &str, done: &mut Vec<String>, busy: &mut Vec<String>) -> PResult<()> {
    if done.iter().any(|x| x == name) {
        return Ok(());
    }
    if busy.iter().any(|x| x == name) {
        return Err(ParseError(format!("`as` references loop through {}", name)));
    }
    busy.push(name.to_string());
    let d = defs.defs[name].clone();
    let (mut needs, mut emits) = (Vec::new(), Vec::new());
    let mut ref_origin = IndexMap::new();
    let mut emit_channel = IndexMap::new();
    let own_channel = d.emit_channel.clone();
    for n in &d.needs {
        let Need::As(callee, args, no) = n else {
            needs.push(n.clone());
            continue;
        };
        let c = callee_of(defs, callee, *no)?;
        expand_one(defs, &c, done, busy)?;
        let cd = &defs.defs[&c];
        let m = bind(cd, callee, args, *no)?;
        for cn in &cd.needs {
            if let Need::Need(cond, ctor, cargs) = cn {
                ref_origin.insert(("need".to_string(), needs.len()), callee.clone());
                needs.push(Need::Need(subst(cond, &m), ctor.clone(), cargs.iter().map(|a| subst(a, &m)).collect()));
            }
        }
    }
    for (i, ev) in d.emits.iter().enumerate() {
        let Emit::As(callee, args, no, extra) = ev else {
            if let Some(ch) = own_channel.get(&i) {
                emit_channel.insert(emits.len(), ch.clone());
            }
            emits.push(ev.clone());
            continue;
        };
        let c = callee_of(defs, callee, *no)?;
        expand_one(defs, &c, done, busy)?;
        let cd = &defs.defs[&c];
        let m = bind(cd, callee, args, *no)?;
        for (j, cev) in cd.emits.iter().enumerate() {
            let Emit::Ev(ctor, cargs, cond) = cev else { continue };
            let c = cond.as_ref().map(|x| subst(x, &m));
            let c = match (extra, c) {
                (Some(x), Some(c)) => Some(Expr::Bin("and", Box::new(x.clone()), Box::new(c))),
                (Some(x), None) => Some(x.clone()),
                (None, c) => c,
            };
            ref_origin.insert(("emit".to_string(), emits.len()), callee.clone());
            if let Some(ch) = cd.emit_channel.get(&j) {
                emit_channel.insert(emits.len(), ch.clone());
            }
            emits.push(Emit::Ev(ctor.clone(), cargs.iter().map(|a| subst(a, &m)).collect(), c));
        }
    }
    let d = defs.defs.get_mut(name).unwrap();
    d.needs = needs;
    d.emits = emits;
    d.ref_origin = ref_origin;
    d.emit_channel = emit_channel;
    busy.retain(|x| x != name);
    done.push(name.to_string());
    Ok(())
}

fn callee_of(defs: &Module, callee: &str, no: usize) -> PResult<String> {
    if !defs.defs.contains_key(callee) {
        return Err(ParseError::at(
            format!("`as {}`: no definition named {}", callee, repr_str(callee)),
            Some(no),
        ));
    }
    Ok(callee.to_string())
}

fn bind(cd: &Def, callee: &str, args: &[Expr], no: usize) -> PResult<IndexMap<String, Expr>> {
    if args.len() != cd.params.len() {
        return Err(ParseError::at(
            format!("`as {}` takes {} argument(s), got {}", callee, cd.params.len(), args.len()),
            Some(no),
        ));
    }
    Ok(cd.params.iter().map(|(p, _)| p.clone()).zip(args.iter().cloned()).collect())
}

/// Parse the collected body lines into statements; `if COND` / `else` open indented blocks.
fn finish(d: &mut Def) -> PResult<()> {
    if d.raw.is_empty() {
        return Ok(());
    }
    let raw = std::mem::take(&mut d.raw);
    let base = raw[0].0;
    d.src_lines.insert(
        "body".to_string(),
        raw.iter()
            .map(|(ind, line, _)| {
                if *ind == base { restore(line) } else { " ".repeat(ind.saturating_sub(base)) + &restore(line) }
            })
            .collect(),
    );
    let (stmts, i) = block(&raw, 0, base)?;
    if i != raw.len() {
        return Err(ParseError::at("unexpected indentation", Some(raw[i].2)));
    }
    d.body = stmts;
    Ok(())
}

fn is_let(line: &str) -> Option<(&str, &str)> {
    // ^[a-z]\w*\s*=(?!=)
    let mut it = line.char_indices();
    match it.next() {
        Some((_, c)) if c.is_ascii_lowercase() => {}
        _ => return None,
    }
    let mut pos = line.len();
    for (i, c) in it {
        if !(c.is_alphanumeric() || c == '_') {
            pos = i;
            break;
        }
    }
    let rest = &line[pos..];
    let after_ws = rest.trim_start();
    if after_ws.starts_with('=') && !after_ws[1..].starts_with('=') {
        let (name, expr) = line.split_once('=').unwrap();
        return Some((name, expr));
    }
    None
}

fn block(raw: &[(usize, String, usize)], mut i: usize, indent: usize) -> PResult<(Vec<Stmt>, usize)> {
    let mut out = Vec::new();
    while i < raw.len() && raw[i].0 == indent {
        let (_, line, no) = &raw[i];
        let no = Some(*no);
        let (word, rest) = partition(line, " ");
        if line == "else" {
            return Err(ParseError::at("`else` without an `if`", no));
        }
        if let Some(m) = for_loop().captures(line) {
            let (inner, j) = sub(raw, i + 1, indent, no)?;
            i = j;
            let invs: Vec<Expr> =
                inner.iter().filter_map(|s| if let Stmt::Inv(e) = s { Some(e.clone()) } else { None }).collect();
            let lead = inner.iter().take_while(|s| matches!(s, Stmt::Inv(_))).count();
            if lead != invs.len() {
                return Err(ParseError::at("loop `invariant` lines must come first in the loop", no));
            }
            let rest_: Vec<Stmt> = inner.into_iter().filter(|s| !matches!(s, Stmt::Inv(_))).collect();
            out.push(Stmt::For(m[1].to_string(), parse_expr(&m[2], no)?, parse_expr(&m[3], no)?, invs, rest_));
            continue;
        }
        if word == "for" && loose_for().is_match(line) {
            return Err(compound_bound(no));
        }
        if word == "invariant" {
            out.push(Stmt::Inv(parse_clause(rest, no)?));
            i += 1;
            continue;
        }
        if word == "match" {
            let (stmt, j) = match_block(raw, i, indent, rest, no)?;
            out.push(stmt);
            i = j;
            continue;
        }
        if word == "if" {
            let cond = parse_expr(rest, no)?;
            let (then, j) = sub(raw, i + 1, indent, no)?;
            i = j;
            let mut other = vec![];
            if i < raw.len() && raw[i].0 == indent && raw[i].1 == "else" {
                let (o, j) = sub(raw, i + 1, indent, Some(raw[i].2))?;
                other = o;
                i = j;
            }
            out.push(Stmt::If(cond, then, other));
            continue;
        }
        if word == "need" {
            let (c, ctor, args) = parse_need(rest, no)?;
            out.push(Stmt::Need(c, ctor, args));
        } else if let Some((name, expr)) = is_let(line) {
            out.push(Stmt::Let(name.trim().to_string(), parse_expr(expr, no)?));
        } else {
            out.push(Stmt::Do(parse_expr(line, no)?));
        }
        i += 1;
    }
    if i < raw.len() && raw[i].0 > indent {
        return Err(ParseError::at("unexpected indentation", Some(raw[i].2)));
    }
    Ok((out, i))
}

/// `match E` at raw[i], then its arms (`PAT -> STMT`, or `PAT ->` and a deeper block), as the `if` chain it stands for
/// (DECISIONS.md, match): `if E = P1 … else if E = P2 … else DEFAULT`. Returns the statement and the next index.
fn match_block(raw: &[(usize, String, usize)], i: usize, indent: usize, subject: &str, no: Option<usize>)
    -> PResult<(Stmt, usize)> {
    let subject = parse_expr(subject, no)?;
    let mut i = i + 1;
    if i >= raw.len() || raw[i].0 <= indent {
        return Err(ParseError::at("`match E` needs indented arms: `PAT -> STMT`", no));
    }
    let arm_indent = raw[i].0;
    let (mut arms, mut default): (Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>) = (vec![], None);
    while i < raw.len() && raw[i].0 == arm_indent {
        let (_, line, ano) = &raw[i];
        let ano = Some(*ano);
        let Some((pat, rest)) = line.split_once("->") else {
            return Err(ParseError::at("a `match` arm is `PAT -> STMT` (or `PAT ->` and an indented block)", ano));
        };
        let (pat, rest) = (pat.trim(), rest.trim());
        let (body, next) = if rest.is_empty() {
            sub(raw, i + 1, arm_indent, ano)?
        } else {
            let (b, _) = block(&[(0, rest.to_string(), ano.unwrap_or(0))], 0, 0)?;
            (b, i + 1)
        };
        i = next;
        if default.is_some() {
            return Err(ParseError::at("`_` must be the last arm of a `match`", ano));
        }
        if pat == "_" {
            default = Some(body);
            continue;
        }
        let value = match pat.strip_prefix('-') {
            Some(d) if !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()) => Expr::Int(-int(d, ano)?),
            _ => parse_expr(pat, ano)?,
        };
        if !matches!(value, Expr::Int(_) | Expr::Text(_) | Expr::Name(_, false)) {
            return Err(ParseError::at(
                format!("a `match` arm names one value (an integer, a constant, a Text or `_`), not {}", repr_str(&restore(pat))),
                ano,
            ));
        }
        if arms.iter().any(|(v, _)| *v == value) {
            return Err(ParseError::at(format!("`match` arm {} appears twice", restore(pat)), ano));
        }
        arms.push((value, body));
    }
    if i < raw.len() && raw[i].0 > indent && raw[i].0 != arm_indent {
        return Err(ParseError::at("unexpected indentation", Some(raw[i].2)));
    }
    if arms.is_empty() {
        return Err(ParseError::at("a `match` with only `_` is just its block: write it without `match`", no));
    }
    let mut chain = default.unwrap_or_default();
    for (value, body) in arms.into_iter().rev() {
        let cond = Expr::Bin("=", Box::new(subject.clone()), Box::new(value));
        chain = vec![Stmt::If(cond, body, chain)];
    }
    Ok((chain.pop().expect("one if"), i))
}

fn sub(raw: &[(usize, String, usize)], i: usize, indent: usize, no: Option<usize>) -> PResult<(Vec<Stmt>, usize)> {
    if i >= raw.len() || raw[i].0 <= indent {
        return Err(ParseError::at("an `if` or `else` needs an indented block", no));
    }
    block(raw, i, raw[i].0)
}

// ---------------------------------------------------------------- printing back to code

pub fn prec(op: &str) -> u8 {
    match op {
        "when" => 0,
        "or" => 1,
        "and" => 2,
        "=" | "!=" | ">=" | "<=" | ">" | "<" => 3,
        "+" | "-" => 4,
        "*" | "/" => 5,
        _ => panic!("no precedence for {}", op),
    }
}

pub fn code(e: &Expr) -> String {
    match e {
        Expr::Int(n) => n.to_string(),
        Expr::Text(t) => quote(t),
        Expr::Name(n, p) => format!("{}{}", n, if *p { "'" } else { "" }),
        Expr::App(h, p, args) => {
            let mut parts = vec![format!("{}{}", h, if *p { "'" } else { "" })];
            parts.extend(args.iter().map(atom_code));
            parts.join(" ")
        }
        Expr::Bin(op, l, r) => format!("{} {} {}", side(l, op, false, code), op, side(r, op, true, code)),
        Expr::All(j, a, b, e) => format!("{} for {} from {} to {}", code(e), j, atom_code(a), atom_code(b)),
        Expr::Sum(j, a, b, x, c) => {
            let s = format!("sum {} from {} to {} of {}", j, atom_code(a), atom_code(b), code(x));
            match c {
                Some(c) => format!("{} where {}", s, code(c)),
                None => s,
            }
        }
    }
}

/// Print a binary operand, parenthesised when needed to reparse to the same tree.
pub fn side(child: &Expr, op: &str, right: bool, show: impl Fn(&Expr) -> String) -> String {
    let text = show(child);
    let (c, p) = match child {
        Expr::Bin(cop, _, _) => (prec(cop), prec(op)),
        // a sum's body (and filter) run to the right, so as an operand it is always parenthesised
        Expr::Sum(..) => return format!("({})", text),
        _ => (99, 0),
    };
    // + - * / and or are left-associative; comparisons and `when` need parentheses on either side
    if c < p || (c == p && (right || CMP.contains(&op) || op == "when")) {
        format!("({})", text)
    } else {
        text
    }
}

pub fn neg_op(op: &str) -> Option<&'static str> {
    Some(match op {
        "=" => "!=",
        "!=" => "=",
        ">=" => "<",
        "<=" => ">",
        ">" => "<=",
        "<" => ">=",
        _ => return None,
    })
}

/// The negation as an expression (De Morgan over and/or), or None if there is none without `not`.
pub fn negate(e: &Expr) -> Option<Expr> {
    let Expr::Bin(op, l, r) = e else { return None };
    if let Some(n) = neg_op(op) {
        return Some(Expr::Bin(n, l.clone(), r.clone()));
    }
    if *op == "and" || *op == "or" {
        let (nl, nr) = (negate(l)?, negate(r)?);
        return Some(Expr::Bin(if *op == "and" { "or" } else { "and" }, Box::new(nl), Box::new(nr)));
    }
    None
}

/// Every statement, including those inside `if` blocks (and the `if` itself) and loop bodies.
pub fn flat(stmts: &[Stmt]) -> Vec<&Stmt> {
    let mut out = Vec::new();
    fn go<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Stmt>) {
        for s in stmts {
            out.push(s);
            match s {
                Stmt::If(_, t, o) => {
                    go(t, out);
                    go(o, out);
                }
                Stmt::For(_, _, _, _, b) => go(b, out),
                _ => {}
            }
        }
    }
    go(stmts, &mut out);
    out
}

pub fn atom_code(e: &Expr) -> String {
    match e {
        Expr::Int(_) | Expr::Text(_) | Expr::Name(..) => code(e),
        _ => format!("({})", code(e)),
    }
}

pub fn emit_code(ev: &Emit) -> String {
    let (head, args, cond) = match ev {
        Emit::Ev(c, a, cond) => (c.clone(), a, cond),
        Emit::As(c, a, _, cond) => (format!("as {}", c), a, cond),
    };
    let mut parts = vec![head];
    parts.extend(args.iter().map(atom_code));
    let text = parts.join(" ");
    match cond {
        Some(c) => format!("{} when {}", text, code(c)),
        None => text,
    }
}

pub fn stmt_code(s: &Stmt) -> String {
    match s {
        Stmt::If(c, _, _) => format!("if {}", code(c)),
        Stmt::For(k, lo, hi, _, _) => format!("for {} from {} to {}", k, atom_code(lo), atom_code(hi)),
        Stmt::Inv(e) => format!("invariant {}", code(e)),
        Stmt::Let(x, e) => format!("{} = {}", x, code(e)),
        Stmt::Need(c, ctor, args) => {
            let mut parts = vec![ctor.clone()];
            parts.extend(args.iter().map(atom_code));
            format!("need {} else {}", code(c), parts.join(" "))
        }
        Stmt::Do(e) => code(e),
    }
}
