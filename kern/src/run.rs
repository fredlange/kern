//! Concrete interpreter (port of kern/run.py), the reference semantics. All effects go through handlers; a
//! top-level call is atomic (failure rolls the world back), so `keep` and `ensure` only have to hold on success.
//!
//! Values are dynamically typed, as in Python: Int, Bool, Str (accounts, owners), constructor values (tuples in
//! Python) and None (what an effect returns). Python's own exceptions that callers catch are kept apart by kind
//! (`RunError::Key`, `RunError::Type`), with Python's message text, so error lines read the same.

use indexmap::{IndexMap, IndexSet};

use crate::pyrandom::Random;
use crate::pyrepr::repr_str;
use crate::syntax::{code, emit_code, Def, Emit, Expr, Key, Module, Need, Stmt};
use crate::Int;

// ---------------------------------------------------------------- values

#[derive(Clone, Debug)]
pub enum Value {
    Int(Int),
    Bool(bool),
    Str(String),
    /// a constructor value / event: ('Ctor', args...) in Python
    Ctor(String, Vec<Value>),
    None,
}

impl Value {
    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Int(n) => *n != 0,
            Value::Bool(b) => *b,
            Value::Str(s) => !s.is_empty(),
            Value::Ctor(..) => true,
            Value::None => false,
        }
    }
    /// Int or Bool as a number (Python's bool is an int).
    pub fn num(&self) -> Option<Int> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Bool(b) => Some(*b as Int),
            _ => None,
        }
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::Ctor(..) => "tuple",
            Value::None => "NoneType",
        }
    }
    /// Python repr.
    pub fn repr(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => (if *b { "True" } else { "False" }).into(),
            Value::Str(s) => repr_str(s),
            Value::None => "None".into(),
            Value::Ctor(c, args) => {
                let mut parts = vec![repr_str(c)];
                parts.extend(args.iter().map(Value::repr));
                if parts.len() == 1 {
                    format!("({},)", parts[0])
                } else {
                    format!("({})", parts.join(", "))
                }
            }
        }
    }
    /// Python str.
    pub fn str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            v => v.repr(),
        }
    }
    pub fn as_key(&self) -> Option<Key> {
        match self {
            Value::Int(n) => Some(Key::Int(*n)),
            Value::Bool(b) => Some(Key::Int(*b as Int)),
            Value::Str(s) => Some(Key::Str(s.clone())),
            _ => None,
        }
    }
    pub fn from_key(k: &Key) -> Value {
        match k {
            Key::Int(n) => Value::Int(*n),
            Key::Str(s) => Value::Str(s.clone()),
        }
    }
}

/// Python ==: bool and int compare as numbers, tuples elementwise, different types are unequal.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Ctor(a, x), Value::Ctor(b, y)) => a == b && x == y,
            (Value::None, Value::None) => true,
            (a, b) => match (a.num(), b.num()) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            },
        }
    }
}

// ---------------------------------------------------------------- errors

#[derive(Clone, Debug, PartialEq)]
pub enum RunError {
    /// KernError: a runtime error of the program (unknown account, a handler need not met, ...)
    Kern(String),
    /// Python KeyError; the text is Python's str(KeyError(k)), i.e. the key's repr
    Key(String),
    /// Python TypeError, with Python's message
    Type(String),
    /// any other Python exception (ZeroDivisionError, ...): callers that catch only the three above let it through
    Other(String, String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            RunError::Kern(m) | RunError::Key(m) | RunError::Type(m) => f.write_str(m),
            RunError::Other(_, m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RunError {}

/// What a body can end in besides success: a declared refusal, or an error.
#[derive(Clone, Debug)]
pub enum Stop {
    Fail(String, Vec<Value>),
    Err(RunError),
}

impl From<RunError> for Stop {
    fn from(e: RunError) -> Stop {
        Stop::Err(e)
    }
}

pub type RResult<T> = Result<T, RunError>;
type SResult<T> = Result<T, Stop>;

fn kern<T>(msg: impl Into<String>) -> Result<T, RunError> {
    Err(RunError::Kern(msg.into()))
}

fn type_err(op: &str, l: &Value, r: &Value) -> RunError {
    RunError::Type(format!("unsupported operand type(s) for {}: '{}' and '{}'", op, l.type_name(), r.type_name()))
}

fn cmp_err(op: &str, l: &Value, r: &Value) -> RunError {
    RunError::Type(format!("'{}' not supported between instances of '{}' and '{}'", op, l.type_name(), r.type_name()))
}

fn int_of(v: &Value, what: &str) -> RResult<Int> {
    v.num().ok_or_else(|| RunError::Type(format!("'{}' object cannot be interpreted as an integer{}", v.type_name(), what)))
}

fn checked(r: Option<Int>) -> RResult<Int> {
    r.ok_or_else(|| RunError::Kern("integer overflow (Kern Int is bounded in this build)".into()))
}

// ---------------------------------------------------------------- the world

/// Where a store-backed world reads what this call hasn't written (the host's SQLite database, inside the call's
/// transaction). Python's LazyWorld.
pub trait Store {
    fn balance(&self, account: &str) -> Option<Int>;
    fn owner(&self, account: &str) -> Option<String>;
    /// the sum of all stored balances
    fn total(&self) -> Int;
    /// a stored `state` cell (None: never written, which reads as 0)
    fn cell(&self, state: &str, key: &Key) -> Option<Int>;
    /// a stored Text `state` cell (None: never written, which reads as ""); a store without Text keeps none
    fn text_cell(&self, _state: &str, _key: &Key) -> Option<String> {
        None
    }
}

/// A shared handle on a Store; copies of a world (the call's snapshot) share it.
#[derive(Clone)]
pub struct Backing(pub std::rc::Rc<dyn Store>);

impl std::fmt::Debug for Backing {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("Backing")
    }
}

impl PartialEq for Backing {
    fn eq(&self, other: &Backing) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Balances, owners, the clock, the audit log, declared state and channel outputs. Every read and write the
/// interpreter makes goes through the methods below. With a `backing`, `bal`, `own` and `maps` hold only what this
/// call wrote (Python's `dirty`), and everything else is read from the store.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct World {
    pub bal: IndexMap<String, Int>,
    pub own: IndexMap<String, String>,
    pub clock: Int,
    pub audit: Vec<Value>,
    /// declared state -> {key: Int}; missing keys are 0
    pub maps: IndexMap<String, IndexMap<Key, Int>>,
    /// declared channel -> events (`audit` stays in .audit)
    pub out: IndexMap<String, Vec<Value>>,
    /// declared Text state -> {key: text}; missing keys are ""
    pub texts: IndexMap<String, IndexMap<Key, String>>,
    pub backing: Option<Backing>,
}

impl World {
    pub fn new(bal: IndexMap<String, Int>, own: IndexMap<String, String>, clock: Int) -> World {
        World { bal, own, clock, ..Default::default() }
    }

    /// A world read from `store` on demand; it starts with nothing written.
    pub fn lazy(store: std::rc::Rc<dyn Store>) -> World {
        World { backing: Some(Backing(store)), ..Default::default() }
    }

    pub fn from_example(w: &IndexMap<String, (Int, String)>, now: Int, maps: &crate::syntax::Maps) -> World {
        World {
            bal: w.iter().map(|(a, (b, _))| (a.clone(), *b)).collect(),
            own: w.iter().map(|(a, (_, o))| (a.clone(), o.clone())).collect(),
            clock: now,
            maps: maps.clone(),
            ..Default::default()
        }
    }

    /// The world with these Text state entries (an `eg` world's `title.1:"…"` items).
    pub fn with_texts(mut self, texts: &crate::syntax::Texts) -> World {
        self.texts = texts.clone();
        self
    }

    pub fn text_get(&self, x: &str, k: &Key) -> String {
        if let Some(v) = self.texts.get(x).and_then(|m| m.get(k)) {
            return v.clone();
        }
        self.backing.as_ref().and_then(|b| b.0.text_cell(x, k)).unwrap_or_default()
    }

    pub fn text_set(&mut self, x: &str, k: Key, v: String) {
        self.texts.entry(x.to_string()).or_default().insert(k, v);
    }

    pub fn events(&self, channel: &str) -> &[Value] {
        if channel == "audit" {
            &self.audit
        } else {
            self.out.get(channel).map(|v| v.as_slice()).unwrap_or(&[])
        }
    }

    fn stored_balance(&self, a: &str) -> Option<Int> {
        self.bal.get(a).copied().or_else(|| self.backing.as_ref().and_then(|b| b.0.balance(a)))
    }

    pub fn has_account(&self, a: &Value) -> bool {
        matches!(a, Value::Str(s) if self.stored_balance(s).is_some())
    }

    fn acct(&self, a: &Value) -> RResult<String> {
        match a {
            Value::Str(s) if s == NONE => kern(format!("{NONE_USED} (an unset entry of an Account-valued state)")),
            Value::Str(s) if self.stored_balance(s).is_some() => Ok(s.clone()),
            _ => kern(format!("unknown account {}", a.repr())),
        }
    }

    pub fn balance(&self, a: &Value) -> RResult<Int> {
        let a = self.acct(a)?;
        Ok(self.stored_balance(&a).unwrap())
    }

    pub fn owner(&self, a: &Value) -> RResult<String> {
        let a = self.acct(a)?;
        self.own
            .get(&a)
            .cloned()
            .or_else(|| self.backing.as_ref().and_then(|b| b.0.owner(&a)))
            .ok_or_else(|| RunError::Key(repr_str(&a)))
    }

    pub fn total(&self) -> RResult<Int> {
        let mut t: Int = 0;
        if let Some(b) = &self.backing {
            // the stored sum, corrected for this call's writes
            t = b.0.total();
            for (a, v) in &self.bal {
                t = checked(t.checked_add(*v - b.0.balance(a).unwrap_or(0)))?;
            }
            return Ok(t);
        }
        for v in self.bal.values() {
            t = checked(t.checked_add(*v))?;
        }
        Ok(t)
    }

    /// `ledger.debit` / `ledger.credit`: add `sign * n` to an account.
    pub fn add_balance(&mut self, a: &Value, n: &Value, sign: Int) -> RResult<()> {
        let a = self.acct(a)?;
        let op = if sign < 0 { "-=" } else { "+=" };
        let Some(n) = n.num() else { return Err(type_err(op, &Value::Int(0), n)) };
        let cur = self.stored_balance(&a).unwrap();
        let v = checked(if sign < 0 { cur.checked_sub(n) } else { cur.checked_add(n) })?;
        self.bal.insert(a, v);
        Ok(())
    }

    pub fn map_get(&self, x: &str, k: &Key) -> Int {
        if let Some(v) = self.maps.get(x).and_then(|m| m.get(k)) {
            return *v;
        }
        self.backing.as_ref().and_then(|b| b.0.cell(x, k)).unwrap_or(0)
    }

    /// {"Account": keys, "Int": keys} this call wrote (a store-backed world), for the key-scoped invariant check.
    pub fn written_keys(&self, states: &IndexMap<String, String>) -> IndexMap<String, IndexSet<Key>> {
        let mut keys: IndexMap<String, IndexSet<Key>> = IndexMap::new();
        keys.insert("Account".into(), self.bal.keys().map(|a| Key::Str(a.clone())).collect());
        keys.insert("Int".into(), IndexSet::new());
        for (x, m) in &self.maps {
            if let Some(t) = states.get(x) {
                keys.entry(t.clone()).or_default().extend(m.keys().cloned());
            }
        }
        for (x, m) in &self.texts {
            if let Some(t) = states.get(x) {
                keys.entry(t.clone()).or_default().extend(m.keys().cloned());
            }
        }
        keys
    }

    pub fn map_set(&mut self, x: &str, k: Key, v: Int) {
        self.maps.entry(x.to_string()).or_default().insert(k, v);
    }

    pub fn push_event(&mut self, channel: &str, ev: Value) {
        if channel == "audit" {
            self.audit.push(ev)
        } else {
            self.out.entry(channel.to_string()).or_default().push(ev)
        }
    }
}

// ---------------------------------------------------------------- handlers

/// op -> (arity, effect label)
/// How deeply calls may nest in the interpreter (one level costs about 5–7 KB of stack: 200 leaves room on a 2 MB
/// thread). Past it a call is a Kern error starting with TOO_DEEP.
pub const MAX_CALL_DEPTH: usize = 200;
pub const TOO_DEEP: &str = "calls nest deeper than";

pub const HANDLERS: [(&str, usize, &str); 8] = [
    ("ledger.balance", 1, "ledger.read"),
    ("ledger.owner", 1, "ledger.read"),
    ("ledger.total", 0, "ledger.read"),
    ("ledger.debit", 2, "ledger.write"),
    ("ledger.credit", 2, "ledger.write"),
    ("clock.now", 0, "clock"),
    ("clock.tick", 0, "clock.tick"),
    ("audit.emit", 1, "audit"),
];

pub fn handler(name: &str) -> Option<(usize, &'static str)> {
    HANDLERS.iter().find(|h| h.0 == name).map(|h| (h.1, h.2))
}

/// `X.get` / `X.set` for a module's `state X Key` declaration: (X, op, key type).
/// What an unset entry of an Account-valued state (`state member Int -> Account`) holds.
pub const NONE: &str = "none";
/// The interpreter's error when `none` is used as an account; the prover rules it out (a call-site goal).
pub const NONE_USED: &str = "none is not an account";

pub fn state_op<'a>(defs: &'a Module, name: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
    let (x, op) = name.rsplit_once('.')?;
    match defs.states.get(x) {
        Some(t) if op == "get" || op == "set" => Some((x, op, t.as_str())),
        _ => None,
    }
}

/// A module's `handler NAME ...` declaration (an observation with an assumed spec).
pub fn assumed_op<'a>(defs: &'a Module, name: &str) -> Option<&'a Def> {
    defs.handlers.get(name)
}

/// `C.emit` for a module's `channel C` declaration: C.
pub fn channel_op<'a>(defs: &Module, name: &'a str) -> Option<&'a str> {
    let (c, op) = name.rsplit_once('.')?;
    (op == "emit" && defs.channels.iter().any(|x| x == c)).then_some(c)
}

pub fn is_handler(defs: &Module, name: &str) -> bool {
    handler(name).is_some()
        || state_op(defs, name).is_some()
        || channel_op(defs, name).is_some()
        || assumed_op(defs, name).is_some()
}

pub fn effect_of(defs: &Module, name: &str) -> String {
    if let Some((_, e)) = handler(name) {
        return e.to_string();
    }
    if assumed_op(defs, name).is_some() {
        return name.split('.').next().unwrap().to_string();
    }
    if let Some(c) = channel_op(defs, name) {
        return c.to_string();
    }
    let (x, op, _) = state_op(defs, name).expect("effect_of: not a handler");
    format!("{}.{}", x, if op == "set" { "write" } else { "read" })
}

/// The channels d's spec says something about, `audit` first; a channel with no clause is unconstrained.
pub fn emit_channels(d: &Def) -> Vec<String> {
    let mut chans: Vec<String> = (0..d.emits.len())
        .map(|i| d.emit_channel.get(&i).cloned().unwrap_or_else(|| "audit".into()))
        .collect();
    chans.sort_by(|a, b| (a != "audit", a).cmp(&(b != "audit", b)));
    chans.dedup();
    chans
}

fn channel_of(d: &Def, i: usize) -> &str {
    d.emit_channel.get(&i).map(|s| s.as_str()).unwrap_or("audit")
}

// ---------------------------------------------------------------- the interpreter

/// Why a provider gave no value: a KernError passes through, anything else becomes "TypeName: message".
#[derive(Clone, Debug)]
pub enum ProvideError {
    Kern(String),
    Other(String, String),
}

/// provider(name, args) -> value for assumed handlers.
pub type Provider<'p> = Box<dyn FnMut(&str, &[Value]) -> Result<Value, ProvideError> + 'p>;

pub struct Interp<'m, 'p> {
    pub defs: &'m Module,
    rng: Random,
    provider: Option<Provider<'p>>,
    /// (name, args, value) per assumed-handler call
    pub observations: Vec<(String, Vec<Value>, Value)>,
    /// replay of loop counterexamples: record loop-invariant violations at iteration heads
    pub check_loops: bool,
    pub loop_violations: Vec<(String, Int)>,
    /// counterexample replay: max loop/quantifier steps (FZ finding 5)
    pub budget: Option<Int>,
    /// the result of the last successful top-level call (functions set it)
    pub last_result: Option<Value>,
    /// when Some, every built-in handler call appends its effect label (tests observe effects with it)
    pub effect_log: Option<Vec<&'static str>>,
    /// the fuzzer's oracle (validation/fuzz): when on, every iteration head of the top-level definition's loops
    /// appends (k, bad) to `heads` instead of checking `check_loops`. `bad` lists the loop invariants false there
    /// (their code() text), then "module invariant TEXT" for each broken module invariant. Nested calls record
    /// nothing.
    pub record_heads: bool,
    pub heads: Vec<(Int, Vec<String>)>,
    /// definitions currently running (1 inside the top-level one)
    depth: usize,
    call_pre: Option<World>,
}

type Env = IndexMap<String, Value>;

impl<'m, 'p> Interp<'m, 'p> {
    pub fn new(defs: &'m Module) -> Self {
        Self::with_provider(defs, None, 0)
    }

    pub fn with_provider(defs: &'m Module, provider: Option<Provider<'p>>, seed: i128) -> Self {
        Interp {
            defs,
            rng: Random::from_int(seed),
            provider,
            observations: vec![],
            check_loops: false,
            loop_violations: vec![],
            budget: None,
            last_result: None,
            effect_log: None,
            record_heads: false,
            heads: vec![],
            depth: 0,
            call_pre: None,
        }
    }

    fn provide(&mut self, name: &str, args: &[Value]) -> Result<Value, ProvideError> {
        if let Some(p) = self.provider.as_mut() {
            return p(name, args);
        }
        if name != "rand.below" {
            return Err(ProvideError::Kern(format!("no provider for assumed handler {}", name)));
        }
        if args.len() != 1 {
            return Err(ProvideError::Other(
                "TypeError".into(),
                format!("<lambda>() takes 2 positional arguments but {} were given", args.len() + 1),
            ));
        }
        let Some(n) = args[0].num() else {
            return Err(ProvideError::Other(
                "TypeError".into(),
                format!("'{}' object cannot be interpreted as an integer", args[0].type_name()),
            ));
        };
        if n <= 0 {
            return Err(ProvideError::Other("ValueError".into(), "empty range for randrange()".into()));
        }
        let n = i64::try_from(n).map_err(|_| ProvideError::Kern("rand.below bound too large".into()))?;
        Ok(Value::Int(self.rng.randrange(0, n) as Int))
    }

    // --- expressions; `pre` is the world unprimed reads see, `post` the one primed reads see

    pub fn ev(&mut self, e: &Expr, env: &Env, pre: &mut World, mut post: Option<&mut World>) -> SResult<Value> {
        match e {
            Expr::Int(n) => Ok(Value::Int(*n)),
            Expr::Text(t) => Ok(Value::Str(t.clone())),
            Expr::All(j, a, b, body) => {
                let lo = self.ev(a, env, pre, post.as_deref_mut())?;
                let hi = self.ev(b, env, pre, post.as_deref_mut())?;
                let (lo, hi) = (int_of(&lo, "")?, int_of(&hi, "")?);
                self.spend(hi - lo)?;
                let mut v = lo;
                while v < hi {
                    let mut e2 = env.clone();
                    e2.insert(j.clone(), Value::Int(v));
                    if !self.ev(body, &e2, pre, post.as_deref_mut())?.truthy() {
                        return Ok(Value::Bool(false));
                    }
                    v += 1;
                }
                Ok(Value::Bool(true))
            }
            Expr::Frame(x, range) => {
                // every key either world holds, outside the range: the same value (missing = the default)
                let bounds = match range {
                    Some((a, b)) => {
                        let lo = int_of(&self.ev(a, env, pre, None)?, "")?;
                        let hi = int_of(&self.ev(b, env, pre, None)?, "")?;
                        Some((lo, hi))
                    }
                    None => None,
                };
                let Some(post) = post else {
                    return Err(RunError::Kern(format!("keep {} (a frame) needs the after-state", code(e))).into());
                };
                let outside = |k: &Key| match (bounds, k) {
                    (Some((lo, hi)), Key::Int(n)) => *n < lo || *n >= hi,
                    _ => bounds.is_none(),
                };
                let text = self.defs.text_states.iter().any(|t| t == x);
                let mut keys: Vec<Key> = vec![];
                for w in [&*pre, &*post] {
                    if text {
                        keys.extend(w.texts.get(x).into_iter().flat_map(|m| m.keys().cloned()));
                    } else {
                        keys.extend(w.maps.get(x).into_iter().flat_map(|m| m.keys().cloned()));
                    }
                }
                let same = keys.iter().filter(|k| outside(k)).all(|k| {
                    if text { pre.text_get(x, k) == post.text_get(x, k) } else { pre.map_get(x, k) == post.map_get(x, k) }
                });
                Ok(Value::Bool(same))
            }
            Expr::Sum(j, a, b, body, cond) => {
                let lo = self.ev(a, env, pre, post.as_deref_mut())?;
                let hi = self.ev(b, env, pre, post.as_deref_mut())?;
                let (lo, hi) = (int_of(&lo, "")?, int_of(&hi, "")?);
                self.spend(hi - lo)?;
                let (mut v, mut total) = (lo, Value::Int(0));
                while v < hi {
                    let mut e2 = env.clone();
                    e2.insert(j.clone(), Value::Int(v));
                    let keep = match cond {
                        Some(c) => self.ev(c, &e2, pre, post.as_deref_mut())?.truthy(),
                        None => true,
                    };
                    if keep {
                        let x = self.ev(body, &e2, pre, post.as_deref_mut())?;
                        total = binop("+", &total, &x)?;
                    }
                    v += 1;
                }
                Ok(total)
            }
            Expr::Bin(op, l, r) => {
                match *op {
                    "and" => {
                        let a = self.ev(l, env, pre, post.as_deref_mut())?;
                        return if a.truthy() { self.ev(r, env, pre, post) } else { Ok(a) };
                    }
                    "or" => {
                        let a = self.ev(l, env, pre, post.as_deref_mut())?;
                        return if a.truthy() { Ok(a) } else { self.ev(r, env, pre, post) };
                    }
                    "when" => {
                        // `E when C`: C reads the before-state
                        if !self.ev(r, env, pre, None)?.truthy() {
                            return Ok(Value::Bool(true));
                        }
                        return Ok(Value::Bool(self.ev(l, env, pre, post)?.truthy()));
                    }
                    _ => {}
                }
                let a = self.ev(l, env, pre, post.as_deref_mut())?;
                let b = self.ev(r, env, pre, post)?;
                Ok(binop(op, &a, &b)?)
            }
            Expr::Name(n, primed) => {
                if let Some(v) = env.get(n) {
                    return Ok(v.clone());
                }
                if n == NONE {
                    return Ok(Value::Str(NONE.into()));
                }
                // `p.x` where p holds a record value (a constructor value, its leaves in order)
                let mut at = n.len();
                while let Some(i) = n[..at].rfind('.') {
                    if let Some(Value::Ctor(r, vals)) = env.get(&n[..i]) {
                        if self.defs.records.contains_key(r) {
                            let rest = &n[i + 1..];
                            if let Some(k) = crate::syntax::record_leaves(&self.defs.records, r).iter().position(|(f, _)| f == rest) {
                                return Ok(vals[k].clone());
                            }
                        }
                    }
                    at = i;
                }
                self.call_in(n, vec![], pre, post, *primed)
            }
            Expr::App(n, primed, args) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.ev(a, env, pre, post.as_deref_mut())?);
                }
                self.call_in(n, vals, pre, post, *primed)
            }
        }
    }

    fn call_in(&mut self, name: &str, args: Vec<Value>, pre: &mut World, post: Option<&mut World>, primed: bool)
        -> SResult<Value> {
        if primed {
            match post {
                Some(w) => self.call(name, args, w),
                None => Err(RunError::Kern(format!("{}' (after-state) only allowed in ensure/keep", name)).into()),
            }
        } else {
            self.call(name, args, pre)
        }
    }

    pub fn call(&mut self, name: &str, args: Vec<Value>, world: &mut World) -> SResult<Value> {
        if let Some((arity, label)) = handler(name) {
            if let Some(log) = self.effect_log.as_mut() {
                log.push(label);
            }
            if args.len() != arity {
                return Err(RunError::Kern(format!("{} takes {} argument(s), got {}", name, arity, args.len())).into());
            }
            return Ok(match name {
                "ledger.balance" => Value::Int(world.balance(&args[0])?),
                "ledger.owner" => Value::Str(world.owner(&args[0])?),
                "ledger.total" => Value::Int(world.total()?),
                "ledger.debit" => {
                    world.add_balance(&args[0], &args[1], -1)?;
                    Value::None
                }
                "ledger.credit" => {
                    world.add_balance(&args[0], &args[1], 1)?;
                    Value::None
                }
                "clock.now" => Value::Int(world.clock),
                "clock.tick" => {
                    // a later instant inside the call (S10): exactly one more, undone with the call on a refusal
                    world.clock = checked(world.clock.checked_add(1))?;
                    Value::None
                }
                "audit.emit" => {
                    world.push_event("audit", args[0].clone());
                    Value::None
                }
                _ => unreachable!(),
            });
        }
        let defs = self.defs;
        if let Some(h) = assumed_op(defs, name) {
            // an observation: a provider supplies it, its spec is checked
            let mut env: Env = h.params.iter().map(|(p, _)| p.clone()).zip(args.iter().cloned()).collect();
            for n in &h.needs {
                // a precondition the caller had to prove: not a refusal
                if let Need::Need(c, _, _) = n {
                    if !self.ev(c, &env, world, None)?.truthy() {
                        return Err(RunError::Kern(format!("need of {} not met: {}", name, code(c))).into());
                    }
                }
            }
            let value = match self.provide(name, &args) {
                Ok(v) => v,
                Err(ProvideError::Kern(m)) => return Err(RunError::Kern(m).into()),
                Err(ProvideError::Other(t, m)) => {
                    let list: Vec<String> = args.iter().map(Value::repr).collect();
                    return Err(RunError::Kern(format!(
                        "the provider of {} gave no value for [{}]: {}: {}",
                        name,
                        list.join(", "),
                        t,
                        m
                    ))
                    .into());
                }
            };
            env.insert("result".into(), value.clone());
            for c in &h.ensures {
                if !self.ev(c, &env, world, None)?.truthy() {
                    return Err(RunError::Kern(format!(
                        "the provider of {} broke its assumed spec: {} with result {}",
                        name,
                        code(c),
                        value.str()
                    ))
                    .into());
                }
            }
            self.observations.push((name.to_string(), args, value.clone()));
            return Ok(value);
        }
        if let Some(ch) = channel_op(defs, name) {
            let ev = args.into_iter().next().ok_or_else(|| RunError::Other("IndexError".into(), "list index out of range".into()))?;
            world.push_event(ch, ev);
            return Ok(Value::None);
        }
        if let Some((x, op, key_type)) = state_op(defs, name) {
            let Some(first) = args.first() else {
                return Err(RunError::Other("IndexError".into(), "list index out of range".into()).into());
            };
            if key_type == "Account" {
                world.acct(first)?;
            }
            let key = first.as_key().ok_or_else(|| RunError::Type(format!("unhashable type: '{}'", first.type_name())))?;
            let text = defs.text_states.iter().any(|t| t == x);
            let account = defs.account_states.iter().any(|t| t == x);
            if op == "get" && account {
                // kept with the Text entries, as a name; unset = none
                let v = world.texts.get(x).and_then(|m| m.get(&key)).cloned();
                let v = v.or_else(|| world.backing.as_ref().and_then(|b| b.0.text_cell(x, &key)));
                return Ok(Value::Str(v.filter(|v| !v.is_empty()).unwrap_or_else(|| NONE.into())));
            }
            if op == "get" {
                return Ok(if text { Value::Str(world.text_get(x, &key)) } else { Value::Int(world.map_get(x, &key)) });
            }
            let Some(v) = args.get(1) else {
                return Err(RunError::Other("IndexError".into(), "list index out of range".into()).into());
            };
            if text || account {
                let Value::Str(t) = v else { return Err(RunError::Type(format!("state {} holds Text, got {}", x, v.repr())).into()) };
                world.text_set(x, key, t.clone());
                return Ok(Value::None);
            }
            let v = v.num().ok_or_else(|| RunError::Type(format!("state {} holds Int, got {}", x, v.repr())))?;
            world.map_set(x, key, v);
            return Ok(Value::None);
        }
        if let Some(d) = defs.defs.get(name) {
            return Ok(self.call_def(d, args, world)?.unwrap_or(Value::None));
        }
        if name.chars().next().map_or(false, |c| c.is_uppercase()) {
            return Ok(Value::Ctor(name.to_string(), args)); // constructor value
        }
        if name == "length" {
            return match args.first() {
                Some(Value::Str(t)) => Ok(Value::Int(t.chars().count() as Int)),
                other => Err(RunError::Type(format!("length of {}", other.map(Value::repr).unwrap_or_default())).into()),
            };
        }
        if name == "distinct" {
            let mut uniq: Vec<&Value> = vec![];
            for a in &args {
                if !uniq.iter().any(|u| *u == a) {
                    uniq.push(a);
                }
            }
            return Ok(Value::Bool(uniq.len() == args.len()));
        }
        Err(RunError::Kern(format!("unknown name {}", repr_str(name))).into())
    }

    /// Run a definition's needs and body; functions (-> Int) return `result`.
    pub fn call_def(&mut self, d: &Def, args: Vec<Value>, world: &mut World) -> SResult<Option<Value>> {
        if self.depth >= MAX_CALL_DEPTH {
            // a resource limit (recursion is licensed by the prover's measure, not by running it), so a 2 MB thread
            // never overflows its stack
            return Err(RunError::Kern(format!("{TOO_DEEP} {MAX_CALL_DEPTH}")).into());
        }
        self.depth += 1;
        let r = self.call_def_body(d, args, world);
        self.depth -= 1;
        r
    }

    fn call_def_body(&mut self, d: &Def, args: Vec<Value>, world: &mut World) -> SResult<Option<Value>> {
        if args.len() != d.params.len() {
            return Err(RunError::Kern(format!("{} takes {} argument(s), got {}", d.name, d.params.len(), args.len())).into());
        }
        if let Some((p, _)) = d.params.iter().zip(&args).find(|((_, t), a)| t == "Account" && **a == Value::Str(NONE.into())).map(|x| x.0) {
            return Err(RunError::Kern(format!("{NONE_USED}: {} got none for `{}`", d.name, p)).into());
        }
        let mut env: Env = d.params.iter().map(|(p, _)| p.clone()).zip(args).collect();
        for n in &d.needs {
            if let Need::Need(c, ctor, cargs) = n {
                self.need(c, ctor, cargs, &env, world)?;
            }
        }
        self.block(&d.body, &mut env, world)?;
        Ok(env.get("result").cloned())
    }

    fn need(&mut self, c: &Expr, ctor: &str, args: &[Expr], env: &Env, world: &mut World) -> SResult<()> {
        if !self.ev(c, env, world, None)?.truthy() {
            let mut vals = vec![];
            for a in args {
                vals.push(self.ev(a, env, world, None)?);
            }
            return Err(Stop::Fail(ctor.to_string(), vals));
        }
        Ok(())
    }

    fn block(&mut self, stmts: &[Stmt], env: &mut Env, world: &mut World) -> SResult<()> {
        for s in stmts {
            match s {
                Stmt::Need(c, ctor, args) => self.need(c, ctor, args, env, world)?,
                Stmt::Let(x, e) => {
                    let v = self.ev(e, env, world, None)?;
                    env.insert(x.clone(), v);
                }
                Stmt::If(c, t, o) => {
                    let branch = if self.ev(c, env, world, None)?.truthy() { t } else { o };
                    self.block(branch, env, world)?;
                }
                Stmt::For(k, lo, hi, _, body) => {
                    // bounds evaluated once; the variable is never assigned
                    let lo = int_of(&self.ev(lo, env, world, None)?, "")?;
                    let hi = int_of(&self.ev(hi, env, world, None)?, "")?;
                    self.spend(hi - lo)?;
                    let mut v = lo;
                    while v < hi {
                        env.insert(k.clone(), Value::Int(v));
                        self.check_loop(s, env, world, v)?;
                        self.block(body, env, world)?;
                        v += 1;
                    }
                    self.check_loop(s, env, world, lo.max(hi))?;
                }
                Stmt::Do(e) | Stmt::Inv(e) => {
                    self.ev(e, env, world, None)?;
                }
            }
        }
        Ok(())
    }

    fn spend(&mut self, n: Int) -> RResult<()> {
        if let Some(b) = self.budget.as_mut() {
            if n > 0 {
                *b -= n;
                if *b < 0 {
                    return kern("replay too long (step budget exceeded)");
                }
            }
        }
        Ok(())
    }

    /// With check_loops on: record a loop invariant (or module invariant) false at an iteration head.
    fn check_loop(&mut self, s: &Stmt, env: &Env, world: &mut World, k: Int) -> SResult<()> {
        if self.record_heads {
            return self.record_head(s, env, world, k);
        }
        if !self.check_loops {
            return Ok(());
        }
        let Stmt::For(var, _, _, invs, _) = s else { return Ok(()) };
        let mut e = env.clone();
        e.insert(var.clone(), Value::Int(k));
        let mut pre = self.call_pre.take().unwrap_or_default();
        let mut res = Ok(());
        for inv in invs {
            match self.ev(inv, &e, &mut pre, Some(world)) {
                Ok(v) if !v.truthy() => self.loop_violations.push((code(inv), k)),
                Ok(_) => {}
                Err(x) => {
                    res = Err(x);
                    break;
                }
            }
        }
        self.call_pre = Some(pre);
        res?;
        for (text, key) in self.broken_invariants_s(world, None)? {
            self.loop_violations.push((format!("module invariant {} at {}", text, key.str()), k));
        }
        Ok(())
    }

    /// With record_heads on: one entry in `heads` per iteration head of the top-level definition.
    fn record_head(&mut self, s: &Stmt, env: &Env, world: &mut World, k: Int) -> SResult<()> {
        if self.depth != 1 {
            return Ok(());
        }
        let Stmt::For(var, _, _, invs, _) = s else { return Ok(()) };
        let mut e = env.clone();
        e.insert(var.clone(), Value::Int(k));
        let mut pre = self.call_pre.take().unwrap_or_default();
        let mut bad = vec![];
        let mut res = Ok(());
        for inv in invs {
            match self.ev(inv, &e, &mut pre, Some(world)) {
                Ok(v) if !v.truthy() => bad.push(code(inv)),
                Ok(_) => {}
                Err(x) => {
                    res = Err(x);
                    break;
                }
            }
        }
        self.call_pre = Some(pre);
        res?;
        for (text, _) in self.broken_invariants_s(world, None)? {
            bad.push(format!("module invariant {text}"));
        }
        self.heads.push((k, bad));
        Ok(())
    }

    /// Atomic top-level call: Ok(outcome) with the world updated (or rolled back on a refusal), or Err with the
    /// world rolled back and no time passed.
    ///
    /// Arguments are checked at the boundary, before anything runs: with the type checker, accounts inside a
    /// body only ever come from parameters, so an unknown account can only enter here.
    pub fn run(&mut self, name: &str, args: &[Value], world: &mut World) -> RResult<Outcome> {
        let defs = self.defs;
        let Some(d) = defs.defs.get(name) else { return Err(RunError::Key(repr_str(name))) };
        if args.len() != d.params.len() {
            return kern(format!("bad call: {} takes {} argument(s), got {}", name, d.params.len(), args.len()));
        }
        for ((p, t), a) in d.params.iter().zip(args) {
            if t == "Account" && !world.has_account(a) {
                return kern(format!("bad call: account {} for `{}` is not in the world", a.repr(), p));
            }
            if t == "Int" && !matches!(a, Value::Int(_)) {
                return kern(format!("bad call: `{}` must be an Int, got {}", p, a.repr()));
            }
            if t == "Text" && !matches!(a, Value::Str(_)) {
                return kern(format!("bad call: `{}` must be a Text, got {}", p, a.repr()));
            }
        }
        for x in &defs.account_states {
            for (k, v) in world.texts.get(x).into_iter().flatten() {
                if v != NONE && !world.has_account(&Value::Str(v.clone())) {
                    return kern(format!("bad world: {}.{} holds {}, which is not an account in the world", x, match k { Key::Int(n) => n.to_string(), Key::Str(a) => a.clone() }, v));
                }
            }
        }
        let snap = world.clone();
        self.call_pre = Some(snap.clone());
        let outcome = match self.call_def(d, args.to_vec(), world) {
            Ok(result) => {
                self.last_result = Some(result.unwrap_or(Value::None));
                Outcome::Ok
            }
            Err(Stop::Fail(c, a)) => {
                *world = snap;
                Outcome::Fail(c, a)
            }
            Err(Stop::Err(e)) => {
                // runtime errors are atomic too: nothing the call did survives (and no time passes)
                *world = snap;
                return Err(e);
            }
        };
        world.clock += 1; // time passes between calls, refused ones included
        Ok(outcome)
    }

    /// Which keep/ensure/emit clauses fail for this (pre, post) pair: [(kind, clause source)].
    pub fn holds(&mut self, d: &Def, args: &[Value], pre: &mut World, post: &mut World) -> RResult<Vec<(String, String)>> {
        let mut env: Env = d.params.iter().map(|(p, _)| p.clone()).zip(args.iter().cloned()).collect();
        if d.ret.is_some() {
            env.insert("result".into(), self.last_result.clone().unwrap_or(Value::None));
        }
        let mut bad = vec![];
        // a clause that uses `none` as an account can't be evaluated: the run broke it (the prover's goal that the
        // account isn't none says the same)
        let none_used = |e: &RunError| matches!(e, RunError::Kern(m) if m.starts_with(NONE_USED));
        for c in &d.keeps {
            let ab = self.ev(c, &env, pre, None).map_err(stop_err).and_then(|a| Ok((a, self.ev(c, &env, post, None).map_err(stop_err)?)));
            match ab {
                Ok((a, b)) if a != b => bad.push(("keep".to_string(), code(c))),
                Ok(_) => {}
                Err(e) if none_used(&e) => bad.push(("keep".to_string(), code(c))),
                Err(e) => return Err(e),
            }
        }
        for c in &d.ensures {
            let kind = if matches!(c, Expr::Frame(..)) { "keep" } else { "ensure" };
            match self.ev(c, &env, pre, Some(post)).map_err(stop_err) {
                Ok(v) if !v.truthy() => bad.push((kind.to_string(), code(c))),
                Ok(_) => {}
                Err(e) if none_used(&e) => bad.push((kind.to_string(), code(c))),
                Err(e) => return Err(e),
            }
        }
        for ch in emit_channels(d) {
            if !self.emitted_ok(d, &env, pre, post, &ch)? {
                let text: Vec<String> = d
                    .emits
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| channel_of(d, *i) == ch)
                    .map(|(_, ev)| emit_code(ev))
                    .collect();
                bad.push(("emit".to_string(), text.join("; ")));
            }
        }
        Ok(bad)
    }

    /// `ok otherwise` is broken by this run: every spec `need` held, yet the call refused.
    pub fn unstated_refusal(&mut self, d: &Def, args: &[Value], before: &World, outcome: &Outcome) -> RResult<bool> {
        if !d.complete || *outcome == Outcome::Ok {
            return Ok(false);
        }
        let env: Env = d.params.iter().map(|(p, _)| p.clone()).zip(args.iter().cloned()).collect();
        let mut w = before.clone();
        for n in &d.needs {
            if let Need::Need(c, _, _) = n {
                if !self.ev(c, &env, &mut w, None).map_err(stop_err)?.truthy() {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Module invariants that some key of this world violates: [(text, key)].
    ///
    /// `keys` = {"Account": [...], "Int": [...]} checks only those keys (the host passes the keys a call wrote).
    pub fn broken_invariants(&mut self, world: &mut World, keys: Option<&IndexMap<String, IndexSet<Key>>>)
        -> RResult<Vec<(String, Value)>> {
        self.broken_invariants_s(world, keys).map_err(stop_err)
    }

    /// As broken_invariants; a refusal raised inside stays a refusal (inside a loop it refuses the call).
    fn broken_invariants_s(&mut self, world: &mut World, keys: Option<&IndexMap<String, IndexSet<Key>>>)
        -> SResult<Vec<(String, Value)>> {
        let mut out = vec![];
        let defs = self.defs;
        for (var, t, cond) in &defs.invariants {
            let keys_t: Vec<Value> = match keys {
                Some(keys) => {
                    let mut ks: Vec<Value> =
                        keys.get(t).map(|s| s.iter().map(Value::from_key).collect()).unwrap_or_default();
                    ks.sort_by_key(|k| k.str());
                    ks
                }
                None if t == "Account" => world.bal.keys().map(|a| Value::Str(a.clone())).collect(),
                None => {
                    let mut ks: Vec<Int> = world
                        .maps
                        .values()
                        .flat_map(|m| m.keys())
                        .filter_map(|k| if let Key::Int(n) = k { Some(*n) } else { None })
                        .collect();
                    ks.sort();
                    ks.dedup();
                    ks.into_iter().map(Value::Int).collect()
                }
            };
            for k in keys_t {
                let mut env = Env::new();
                env.insert(var.clone(), k.clone());
                if !self.ev(cond, &env, world, None)?.truthy() {
                    out.push((format!("{}:{} {}", var, t, code(cond)), k));
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The call appended exactly the events its `emit` clauses list for this channel, in order; `_` matches
    /// anything.
    pub fn emitted_ok(&mut self, d: &Def, env: &Env, pre: &World, post: &World, channel: &str) -> RResult<bool> {
        let (before, after) = (pre.events(channel), post.events(channel));
        let mut want: Vec<&Emit> = vec![];
        for (i, ev) in d.emits.iter().enumerate() {
            if channel_of(d, i) != channel {
                continue;
            }
            match ev.cond() {
                None => want.push(ev),
                Some(c) => {
                    if self.ev(c, env, &mut pre.clone(), None).map_err(stop_err)?.truthy() {
                        want.push(ev);
                    }
                }
            }
        }
        if after.len() < before.len() || after[..before.len()] != *before {
            return Ok(false);
        }
        let got = &after[before.len()..];
        if got.len() != want.len() {
            return Ok(false);
        }
        for (ev, v) in want.iter().zip(got) {
            let (Emit::Ev(ctor, args, _) | Emit::As(ctor, args, _, _)) = ev;
            let Value::Ctor(vc, vargs) = v else { return Ok(false) };
            if vc != ctor || vargs.len() != args.len() {
                return Ok(false);
            }
            for (a, x) in args.iter().zip(vargs) {
                if *a != Expr::Name("_".into(), false) && self.ev(a, env, &mut pre.clone(), None).map_err(stop_err)? != *x {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// A refusal raised while evaluating a spec clause (a `need` inside a called function) is a Python `Fail`
/// exception escaping: report it as an error with Python's text (the constructor name).
fn stop_err(s: Stop) -> RunError {
    match s {
        Stop::Err(e) => e,
        Stop::Fail(c, _) => RunError::Other("Fail".into(), c),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Ok,
    Fail(String, Vec<Value>),
}

impl Outcome {
    /// `ok`, or `Ctor arg ...` (args as Python str).
    pub fn text(&self) -> String {
        match self {
            Outcome::Ok => "ok".into(),
            Outcome::Fail(c, a) => {
                let mut parts = vec![c.clone()];
                parts.extend(a.iter().map(Value::str));
                parts.join(" ")
            }
        }
    }
}

fn floordiv(a: Int, b: Int) -> RResult<Int> {
    if b == 0 {
        return Err(RunError::Other("ZeroDivisionError".into(), "integer division or modulo by zero".into()));
    }
    let q = checked(a.checked_div(b))?;
    Ok(if (a % b != 0) && ((a < 0) != (b < 0)) { q - 1 } else { q })
}

fn binop(op: &str, a: &Value, b: &Value) -> RResult<Value> {
    if let ("+", Value::Str(s), Value::Str(t)) = (op, a, b) {
        return Ok(Value::Str(format!("{s}{t}"))); // Text + Text joins them
    }
    let (x, y) = (a.num(), b.num());
    match op {
        "=" => return Ok(Value::Bool(a == b)),
        "!=" => return Ok(Value::Bool(a != b)),
        _ => {}
    }
    let (Some(x), Some(y)) = (x, y) else {
        return Err(match op {
            "+" | "-" | "*" => type_err(op, a, b),
            "/" => type_err("//", a, b),
            _ => match (a, b) {
                (Value::Str(s), Value::Str(t)) => {
                    return Ok(Value::Bool(match op {
                        ">=" => s >= t,
                        "<=" => s <= t,
                        ">" => s > t,
                        _ => s < t,
                    }))
                }
                _ => cmp_err(op, a, b),
            },
        });
    };
    Ok(match op {
        "+" => Value::Int(checked(x.checked_add(y))?),
        "-" => Value::Int(checked(x.checked_sub(y))?),
        "*" => Value::Int(checked(x.checked_mul(y))?),
        "/" => Value::Int(floordiv(x, y)?),
        ">=" => Value::Bool(x >= y),
        "<=" => Value::Bool(x <= y),
        ">" => Value::Bool(x > y),
        "<" => Value::Bool(x < y),
        _ => return Err(RunError::Kern(format!("cannot evaluate operator {}", op))),
    })
}
