//! kern-rt: the hand-written half of every compiled Kern program.
//!
//! `kern/emit_rust.py` turns a Kern module into Rust functions that run against a [`Tx`]; this
//! crate owns everything else: the world, atomic calls (undo log), checked `Int` arithmetic, the
//! clock, assumed-handler providers with recording, and the driver. The semantics are those of the
//! reference interpreter `kern/run.py`; docs/research/RUNTIME_PLAN.md lists the contract and the one
//! deliberate divergence (Int is i64; leaving it is a fault).
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasher;

pub mod driver;

/// An account, interned at the boundary: the index into the world's vectors.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Acct(pub u32);

/// An account owner, interned.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Owner(pub u32);

/// A runtime error: atomic like a refusal, but not a declared outcome, and no time passes.
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    Overflow,
    Error(String),
}

/// Why a call stopped early.
#[derive(Debug, PartialEq)]
pub enum Stop<R> {
    Refused(R),
    Fault(Fault),
}

impl<R> From<Fault> for Stop<R> {
    fn from(f: Fault) -> Self {
        Stop::Fault(f)
    }
}

// ---- Int: proven as a mathematical integer, run as i64; leaving i64 is a fault

#[inline(always)]
pub fn add(a: i64, b: i64) -> Result<i64, Fault> {
    a.checked_add(b).ok_or(Fault::Overflow)
}
#[inline(always)]
pub fn sub(a: i64, b: i64) -> Result<i64, Fault> {
    a.checked_sub(b).ok_or(Fault::Overflow)
}
#[inline(always)]
pub fn mul(a: i64, b: i64) -> Result<i64, Fault> {
    a.checked_mul(b).ok_or(Fault::Overflow)
}
/// `/` by a positive literal (the type checker guarantees it): rounds toward minus infinity,
/// like Z3 and Python. Rust's `/` rounds toward zero.
#[inline(always)]
pub fn div(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/// A call argument at the boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Int(i64),
    Acct(Acct),
    Text(String),
}

/// A parameter's type (and a map's key type: Int or Account only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    Int,
    Account,
    Text,
}

/// What the compiler knows about a definition.
pub struct DefInfo {
    pub name: &'static str,
    pub hash: &'static str,
    pub params: &'static [(&'static str, KeyType)],
    pub ret: bool,
    pub uses: &'static [&'static str],
}

/// A declared `state NAME KeyType [-> Text]`; `slot` indexes the account or Int maps of its value type (Int maps and
/// Text maps are numbered apart).
pub struct MapInfo {
    pub name: &'static str,
    pub key: KeyType,
    pub slot: usize,
    pub text: bool,
}

// ---- account names: stored once, in one byte arena, with an index that holds only ids

const EMPTY: u32 = u32::MAX;

/// Interned account names. A `Vec<String>` plus a `HashMap<String, u32>` stored every name twice
/// and cost about 160 bytes per account (docs/research/RUNTIME_PLAN.md, M1); this costs the name's bytes
/// plus about 4 (offset) + 5-8 (open-addressing slots, load <= 3/4) bytes. The hash is keyed per
/// process (SipHash via `RandomState`), so names sent by clients can't be chosen to collide.
pub struct Names {
    bytes: Vec<u8>,
    ends: Vec<u32>,
    slots: Vec<u32>,
    hasher: RandomState,
}

impl Default for Names {
    fn default() -> Self {
        Names { bytes: Vec::new(), ends: Vec::new(), slots: Vec::new(), hasher: RandomState::new() }
    }
}

impl Names {
    pub fn len(&self) -> usize {
        self.ends.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }
    pub fn get(&self, id: u32) -> &str {
        let i = id as usize;
        let start = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        // only whole `&str`s are ever appended, so every slice between two ends is UTF-8
        std::str::from_utf8(&self.bytes[start..self.ends[i] as usize]).expect("names are UTF-8")
    }
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        (0..self.ends.len() as u32).map(move |i| self.get(i))
    }
    fn slot_of(&self, name: &str) -> usize {
        self.hasher.hash_one(name) as usize & (self.slots.len() - 1)
    }
    pub fn find(&self, name: &str) -> Option<u32> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut i = self.slot_of(name);
        loop {
            let id = self.slots[i];
            if id == EMPTY {
                return None;
            }
            if self.get(id) == name {
                return Some(id);
            }
            i = (i + 1) & mask;
        }
    }
    /// Add a name that is not present yet; returns its id.
    pub fn push(&mut self, name: &str) -> u32 {
        debug_assert!(self.find(name).is_none());
        let id = self.ends.len() as u32;
        self.bytes.extend_from_slice(name.as_bytes());
        self.ends.push(u32::try_from(self.bytes.len()).expect("name arena over 4 GiB"));
        if (self.ends.len()) * 4 > self.slots.len() * 3 {
            self.rehash((self.slots.len() * 2).max(16));
        } else {
            self.place(id);
        }
        id
    }
    fn place(&mut self, id: u32) {
        let mask = self.slots.len() - 1;
        let mut i = self.slot_of(self.get(id));
        while self.slots[i] != EMPTY {
            i = (i + 1) & mask;
        }
        self.slots[i] = id;
    }
    fn rehash(&mut self, size: usize) {
        self.slots = vec![EMPTY; size];
        for id in 0..self.ends.len() as u32 {
            self.place(id);
        }
    }
    /// Reserve room for `n` more names of about `avg_len` bytes (a snapshot load knows both).
    pub fn reserve(&mut self, n: usize, avg_len: usize) {
        self.bytes.reserve(n * avg_len);
        self.ends.reserve(n);
        let want = ((self.ends.len() + n) * 4 / 3 + 1).next_power_of_two().max(16);
        if want > self.slots.len() {
            self.rehash(want);
        }
    }
}

// ---- the world

pub struct World {
    pub bal: Vec<i64>,
    pub owner: Vec<Owner>,
    /// account names, stored once (see [`Names`])
    pub names: Names,
    pub owners: Vec<String>,
    pub owner_index: HashMap<String, u32>,
    /// `state X Account`: dense, one entry per account; missing = 0
    pub amaps: Vec<Vec<i64>>,
    /// `state X Int`: sparse; missing = 0
    pub imaps: Vec<HashMap<i64, i64>>,
    /// `state X Account -> Text`: sparse by account id; missing = ""
    pub atexts: Vec<HashMap<u32, String>>,
    /// `state X Int -> Text`: sparse; missing = ""
    pub itexts: Vec<HashMap<i64, String>>,
    pub clock: i64,
}

impl World {
    pub fn new(n_amaps: usize, n_imaps: usize) -> Self {
        World {
            bal: Vec::new(),
            owner: Vec::new(),
            names: Names::default(),
            owners: Vec::new(),
            owner_index: HashMap::new(),
            amaps: vec![Vec::new(); n_amaps],
            imaps: vec![HashMap::new(); n_imaps],
            atexts: Vec::new(),
            itexts: Vec::new(),
            clock: 0,
        }
    }

    /// Reserve room for `n` more accounts (a snapshot load knows how many).
    pub fn reserve(&mut self, n: usize, avg_name_len: usize) {
        self.bal.reserve(n);
        self.owner.reserve(n);
        self.names.reserve(n, avg_name_len);
        for m in self.amaps.iter_mut() {
            m.reserve(n);
        }
    }

    pub fn acct(&self, name: &str) -> Option<Acct> {
        self.names.find(name).map(Acct)
    }

    pub fn name(&self, a: Acct) -> &str {
        self.names.get(a.0)
    }

    pub fn owner_name(&self, o: Owner) -> &str {
        &self.owners[o.0 as usize]
    }

    /// Open an account (or reset an existing one's balance and owner).
    pub fn open(&mut self, name: &str, balance: i64, owner: &str) -> Acct {
        let o = match self.owner_index.get(owner) {
            Some(&o) => Owner(o),
            None => {
                let o = self.owners.len() as u32;
                self.owners.push(owner.to_string());
                self.owner_index.insert(owner.to_string(), o);
                Owner(o)
            }
        };
        if let Some(a) = self.acct(name) {
            self.bal[a.0 as usize] = balance;
            self.owner[a.0 as usize] = o;
            return a;
        }
        let id = self.bal.len() as u32;
        self.bal.push(balance);
        self.owner.push(o);
        self.names.push(name);
        for m in self.amaps.iter_mut() {
            m.push(0);
        }
        Acct(id)
    }
}

// ---- assumed handlers: a provider supplies the value, the generated code checks the spec

pub trait Provider {
    fn provide(&mut self, name: &str, args: &[Val]) -> Result<i64, String>;
}

/// Values recorded earlier (replay, and the differential test): handed out in order.
#[derive(Default)]
pub struct Replay(pub VecDeque<i64>);

impl Provider for Replay {
    fn provide(&mut self, name: &str, args: &[Val]) -> Result<i64, String> {
        self.0.pop_front().ok_or_else(|| format!("replay has no value for {name} {args:?}"))
    }
}

/// Seeded pseudo-random values for `rand.below n` (not the same stream as Python's: replay and
/// the differential test inject recorded values instead).
pub struct Seeded(pub u64);

impl Provider for Seeded {
    fn provide(&mut self, name: &str, args: &[Val]) -> Result<i64, String> {
        match (name, args) {
            ("rand.below", [Val::Int(n)]) if *n > 0 => {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                Ok((self.0 % (*n as u64)) as i64)
            }
            _ => Err(format!("no value for {name} {args:?}")),
        }
    }
}

/// One recorded observation: (handler, args, value).
pub type Obs = (&'static str, Vec<Val>, i64);

// ---- calls

enum Undo {
    Bal(u32, i64),
    A(u16, u32, i64),
    I(u16, i64, Option<i64>),
    TA(u16, u32, Option<String>),
    TI(u16, i64, Option<String>),
}

/// Loop and sum iterations one top-level call may start (S21), charged as the interpreter charges its budget: `hi - lo`
/// when a loop or sum begins. kern host uses the same number (host.rs ITERATION_LIMIT).
pub const ITERATION_LIMIT: i64 = 10_000_000;

/// How deeply definitions may nest in one call, the top-level one included: the interpreter's MAX_CALL_DEPTH
/// (run.rs). Recursion is licensed by the prover's measure; this is a resource limit, the same in both.
pub const MAX_CALL_DEPTH: u32 = 200;

/// Reusable per-thread buffers, so a call allocates nothing in the steady state.
pub struct Exec<E> {
    undo: Vec<Undo>,
    /// (channel index, event) for the last call; channel 0 is `audit`
    pub events: Vec<(u16, E)>,
    pub obs: Vec<Obs>,
    /// iterations per top-level call (ITERATION_LIMIT unless changed)
    pub limit: i64,
}

impl<E> Default for Exec<E> {
    fn default() -> Self {
        Exec { undo: Vec::new(), events: Vec::new(), obs: Vec::new(), limit: ITERATION_LIMIT }
    }
}

/// One call in progress. Writes go straight to the world and are recorded for rollback.
pub struct Tx<'a, E> {
    w: &'a mut World,
    undo: &'a mut Vec<Undo>,
    events: &'a mut Vec<(u16, E)>,
    obs: &'a mut Vec<Obs>,
    prov: &'a mut dyn Provider,
    /// iterations this call may still start
    budget: i64,
    /// definitions entered and not yet returned from
    depth: u32,
    /// `clock.now`: one value for the whole top-level call
    pub now: i64,
}

impl<'a, E> Tx<'a, E> {
    #[inline(always)]
    pub fn balance(&self, a: Acct) -> i64 {
        self.w.bal[a.0 as usize]
    }
    #[inline(always)]
    pub fn owner(&self, a: Acct) -> Owner {
        self.w.owner[a.0 as usize]
    }
    pub fn total(&self) -> Result<i64, Fault> {
        self.w.bal.iter().try_fold(0i64, |s, &b| add(s, b))
    }
    #[inline(always)]
    fn set_bal(&mut self, a: Acct, v: i64) {
        let i = a.0 as usize;
        self.undo.push(Undo::Bal(a.0, self.w.bal[i]));
        self.w.bal[i] = v;
    }
    #[inline(always)]
    pub fn debit(&mut self, a: Acct, n: i64) -> Result<(), Fault> {
        let v = sub(self.balance(a), n)?;
        self.set_bal(a, v);
        Ok(())
    }
    #[inline(always)]
    pub fn credit(&mut self, a: Acct, n: i64) -> Result<(), Fault> {
        let v = add(self.balance(a), n)?;
        self.set_bal(a, v);
        Ok(())
    }
    #[inline(always)]
    pub fn aget(&self, slot: usize, a: Acct) -> i64 {
        self.w.amaps[slot][a.0 as usize]
    }
    #[inline(always)]
    pub fn aset(&mut self, slot: usize, a: Acct, v: i64) {
        let cell = &mut self.w.amaps[slot][a.0 as usize];
        self.undo.push(Undo::A(slot as u16, a.0, *cell));
        *cell = v;
    }
    #[inline(always)]
    pub fn iget(&self, slot: usize, k: i64) -> i64 {
        *self.w.imaps[slot].get(&k).unwrap_or(&0)
    }
    #[inline(always)]
    pub fn iset(&mut self, slot: usize, k: i64, v: i64) {
        let old = self.w.imaps[slot].insert(k, v);
        self.undo.push(Undo::I(slot as u16, k, old));
    }
    /// Text maps: a missing entry reads "" (a clone: the caller owns its Text)
    pub fn atget(&self, slot: usize, a: Acct) -> String {
        self.w.atexts[slot].get(&a.0).cloned().unwrap_or_default()
    }
    pub fn atset(&mut self, slot: usize, a: Acct, v: String) {
        let old = self.w.atexts[slot].insert(a.0, v);
        self.undo.push(Undo::TA(slot as u16, a.0, old));
    }
    pub fn itget(&self, slot: usize, k: i64) -> String {
        self.w.itexts[slot].get(&k).cloned().unwrap_or_default()
    }
    pub fn itset(&mut self, slot: usize, k: i64, v: String) {
        let old = self.w.itexts[slot].insert(k, v);
        self.undo.push(Undo::TI(slot as u16, k, old));
    }
    #[inline(always)]
    pub fn emit(&mut self, channel: u16, e: E) {
        self.events.push((channel, e));
    }
    /// Ask the provider for an assumed handler's value. The generated caller checks the handler's
    /// needs before and its ensures after, then calls [`Tx::record`].
    pub fn provide(&mut self, name: &'static str, args: &[Val]) -> Result<i64, Fault> {
        self.prov
            .provide(name, args)
            .map_err(|e| Fault::Error(format!("the provider of {name} gave no value for {args:?}: {e}")))
    }
    /// Record an observation once its assumed spec held (run.py records at the same point).
    pub fn record(&mut self, name: &'static str, args: &[Val], v: i64) {
        self.obs.push((name, args.to_vec(), v));
    }
    /// The accounts this call wrote so far (balances and Account maps; a write of an unchanged value counts), each
    /// once, in first-write order: where a module invariant over `Account` keys is checked.
    pub fn written_accts(&self) -> Vec<Acct> {
        // a set beside the list: a call may write a million keys (nested loops), so no linear search
        let (mut out, mut seen): (Vec<Acct>, std::collections::HashSet<u32>) = (Vec::new(), Default::default());
        for u in self.undo.iter() {
            let a = match u {
                Undo::Bal(i, _) | Undo::A(_, i, _) => *i,
                Undo::TA(_, i, _) => *i,
                Undo::I(..) | Undo::TI(..) => continue,
            };
            if seen.insert(a) {
                out.push(Acct(a));
            }
        }
        out
    }
    /// The Int keys this call wrote so far (Int maps), each once, in first-write order.
    pub fn written_ints(&self) -> Vec<i64> {
        let (mut out, mut seen): (Vec<i64>, std::collections::HashSet<i64>) = (Vec::new(), Default::default());
        for u in self.undo.iter() {
            if let Undo::I(_, k, _) | Undo::TI(_, k, _) = u {
                if seen.insert(*k) {
                    out.push(*k);
                }
            }
        }
        out
    }
    /// A definition starts: one level deeper, a fault past MAX_CALL_DEPTH (as the interpreter's Kern error).
    #[inline(always)]
    pub fn enter(&mut self) -> Result<(), Fault> {
        if self.depth >= MAX_CALL_DEPTH {
            return Err(Fault::Error(format!("calls nest deeper than {MAX_CALL_DEPTH}")));
        }
        self.depth += 1;
        Ok(())
    }
    /// A definition returned normally (a refusal or fault ends the whole call, so it never needs this).
    #[inline(always)]
    pub fn leave(&mut self) {
        self.depth -= 1;
    }
    /// `clock.tick`: a later instant in the same call, exactly one more (S10), as the interpreter's checked add.
    pub fn tick(&mut self) -> Result<(), Fault> {
        self.now = self.now.checked_add(1).ok_or(Fault::Overflow)?;
        Ok(())
    }
    /// A loop or sum over [lo, hi) is about to start: charge hi - lo against the call's iteration limit (S21).
    #[inline(always)]
    pub fn spend(&mut self, lo: i64, hi: i64) -> Result<(), Fault> {
        let n = hi as i128 - lo as i128;
        if n > 0 {
            self.budget = (self.budget as i128 - n).max(-1) as i64;
            if self.budget < 0 {
                return Err(Fault::Error("iteration limit exceeded".into()));
            }
        }
        Ok(())
    }
    /// An account's name (for a fault message).
    pub fn acct_name(&self, a: Acct) -> &str {
        self.w.name(a)
    }
}

impl<E> Exec<E> {
    /// An atomic top-level call. A refusal or fault undoes every write and drops the events.
    /// Time passes after `ok` and after a refusal, not after a fault (the call did not happen).
    pub fn call<T, R>(
        &mut self,
        w: &mut World,
        prov: &mut dyn Provider,
        body: impl FnOnce(&mut Tx<E>) -> Result<T, Stop<R>>,
    ) -> Result<T, Stop<R>> {
        self.undo.clear();
        self.events.clear();
        self.obs.clear();
        let now = w.clock;
        let (out, end) = {
            let budget = self.limit;
            let mut tx = Tx { w: &mut *w, undo: &mut self.undo, events: &mut self.events, obs: &mut self.obs, prov, budget, depth: 0, now };
            let out = body(&mut tx);
            (out, tx.now)
        };
        if out.is_ok() {
            w.clock = end; // the call's clock.ticks stay; a refusal or fault undoes them with everything else
        }
        if out.is_err() {
            while let Some(u) = self.undo.pop() {
                match u {
                    Undo::Bal(i, v) => w.bal[i as usize] = v,
                    Undo::A(m, i, v) => w.amaps[m as usize][i as usize] = v,
                    Undo::I(m, k, v) => match v {
                        Some(v) => {
                            w.imaps[m as usize].insert(k, v);
                        }
                        None => {
                            w.imaps[m as usize].remove(&k);
                        }
                    },
                    Undo::TA(m, a, v) => match v {
                        Some(v) => {
                            w.atexts[m as usize].insert(a, v);
                        }
                        None => {
                            w.atexts[m as usize].remove(&a);
                        }
                    },
                    Undo::TI(m, k, v) => match v {
                        Some(v) => {
                            w.itexts[m as usize].insert(k, v);
                        }
                        None => {
                            w.itexts[m as usize].remove(&k);
                        }
                    },
                }
            }
            self.events.clear();
        }
        if !matches!(out, Err(Stop::Fault(_))) {
            w.clock += 1;
        }
        out
    }
}

/// The interface a generated module implements; the driver (and later the host) is generic over it.
pub trait Module {
    type Refusal: std::fmt::Debug;
    type Event: Clone + std::fmt::Debug;
    const NAME: &'static str;
    const DEFS: &'static [DefInfo];
    const MAPS: &'static [MapInfo];
    /// Index 0 is `audit`; then the declared channels in order.
    const CHANNELS: &'static [&'static str];
    fn call(
        exec: &mut Exec<Self::Event>,
        w: &mut World,
        prov: &mut dyn Provider,
        def: usize,
        args: &[Val],
    ) -> Result<Option<i64>, Stop<Self::Refusal>>;
    fn show_refusal(r: &Self::Refusal, w: &World) -> String;
    fn show_event(e: &Self::Event, w: &World) -> String;

    fn new_world() -> World {
        let count = |key: KeyType, text: bool| Self::MAPS.iter().filter(|m| m.key == key && m.text == text).count();
        let mut w = World::new(count(KeyType::Account, false), count(KeyType::Int, false));
        w.atexts = vec![HashMap::new(); count(KeyType::Account, true)];
        w.itexts = vec![HashMap::new(); count(KeyType::Int, true)];
        w
    }
}

/// Python's `str()` of a Bool, so canonical output matches the reference.
pub fn show_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

/// Argument unpacking for the generated dispatch (the driver checked the types).
#[inline(always)]
pub fn acct(v: &Val) -> Acct {
    match v {
        Val::Acct(a) => *a,
        _ => unreachable!("checked at the boundary"),
    }
}
#[inline(always)]
pub fn int(v: &Val) -> i64 {
    match v {
        Val::Int(n) => *n,
        _ => unreachable!("checked at the boundary"),
    }
}
pub fn text(v: &Val) -> String {
    match v {
        Val::Text(t) => t.clone(),
        _ => unreachable!("checked at the boundary"),
    }
}

/// `length t`: the number of characters (code points), as the interpreter counts.
pub fn length(t: &str) -> i64 {
    t.chars().count() as i64
}

/// A Text as Kern source writes it: quoted, `"` and `\` escaped (the driver's dump and world lines).
pub fn quote(t: &str) -> String {
    format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Split a script line on whitespace outside `"…"` (escapes `\"` and `\\`); a quoted token keeps its quotes.
pub fn tokens(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut esc) = (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        if quoted {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            if c == '"' {
                quoted = true;
            }
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The Text inside a quoted token (`"a \"b\""` -> `a "b"`); None if it isn't one.
pub fn unquote(tok: &str) -> Option<String> {
    let inner = tok.strip_prefix('"')?.strip_suffix('"')?;
    let (mut out, mut esc) = (String::new(), false);
    for c in inner.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return None;
        } else {
            out.push(c);
        }
    }
    (!esc).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_find_every_name_and_only_those() {
        let mut n = Names::default();
        for i in 0..200_000u32 {
            assert_eq!(n.push(&format!("A{i}")), i);
        }
        for i in 0..200_000u32 {
            assert_eq!(n.find(&format!("A{i}")), Some(i));
            assert_eq!(n.get(i), format!("A{i}"));
        }
        for miss in ["A200000", "B0", "", "A-1", "a0"] {
            assert_eq!(n.find(miss), None, "{miss}");
        }
        assert_eq!(n.iter().count(), 200_000);
    }

    #[test]
    fn names_handle_empty_and_unicode_and_reserve() {
        let mut n = Names::default();
        assert_eq!(n.find("x"), None);
        n.reserve(10, 4);
        let ids: Vec<u32> = ["", "Å", "kärna", "日本"].iter().map(|s| n.push(s)).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
        assert_eq!((n.find(""), n.find("kärna"), n.get(3)), (Some(0), Some(2), "日本"));
    }

    #[test]
    fn written_keys_come_from_the_undo_log_once_each() {
        let mut w = World::new(1, 1);
        let a = w.open("A", 5, "u1");
        let b = w.open("B", 7, "u2");
        let mut exec: Exec<()> = Exec::default();
        let mut prov = Replay::default();
        let seen = exec.call(&mut w, &mut prov, |tx| -> Result<_, Stop<()>> {
            assert!(tx.written_accts().is_empty() && tx.written_ints().is_empty());
            tx.credit(b, 1)?;
            tx.aset(0, a, 0); // an unchanged value is still a write
            tx.debit(b, 1)?;
            tx.iset(0, 7, 1);
            tx.iset(0, -2, 0);
            tx.iset(0, 7, 3);
            let names: Vec<String> = tx.written_accts().iter().map(|x| tx.acct_name(*x).to_string()).collect();
            Ok((names, tx.written_ints()))
        });
        assert_eq!(seen, Ok((vec!["B".to_string(), "A".to_string()], vec![7, -2])));
    }

    #[test]
    fn the_iteration_limit_is_per_call_and_cumulative() {
        let mut w = World::new(0, 0);
        let mut exec: Exec<()> = Exec { limit: 10, ..Default::default() };
        let mut prov = Replay::default();
        let mut run = |exec: &mut Exec<()>, spans: &[(i64, i64)]| {
            let spans = spans.to_vec();
            exec.call(&mut w, &mut prov, move |tx| -> Result<(), Stop<()>> {
                for (lo, hi) in spans {
                    tx.spend(lo, hi)?;
                }
                Ok(())
            })
        };
        assert_eq!(run(&mut exec, &[(0, 4), (5, 3), (2, 8)]), Ok(())); // 4 + 0 + 6 = 10
        assert!(matches!(run(&mut exec, &[(0, 4), (0, 7)]), Err(Stop::Fault(_)))); // 11
        assert_eq!(run(&mut exec, &[(0, 10)]), Ok(()), "a new call starts with the whole limit");
        assert!(matches!(run(&mut exec, &[(i64::MIN, i64::MAX)]), Err(Stop::Fault(_))));
    }

    #[test]
    fn text_writes_roll_back_and_count_as_written() {
        let mut w = World::new(0, 0);
        w.atexts = vec![HashMap::new()];
        w.itexts = vec![HashMap::new()];
        let a = w.open("A", 0, "u1");
        w.itexts[0].insert(1, "old".into());
        let mut exec: Exec<()> = Exec::default();
        let mut prov = Replay::default();
        let r = exec.call(&mut w, &mut prov, |tx| -> Result<(), Stop<()>> {
            tx.itset(0, 1, "new".into());
            tx.itset(0, 2, "fresh".into());
            tx.atset(0, a, "tag".into());
            assert_eq!((tx.itget(0, 1), tx.itget(0, 9), tx.atget(0, a)), ("new".into(), String::new(), "tag".into()));
            assert_eq!((tx.written_ints(), tx.written_accts().len()), (vec![1, 2], 1));
            Err(Stop::Refused(()))
        });
        assert_eq!(r, Err(Stop::Refused(())));
        assert_eq!(w.itexts[0], HashMap::from([(1, "old".to_string())]), "undone, the new key removed");
        assert!(w.atexts[0].is_empty());
    }

    #[test]
    fn script_tokens_keep_quoted_text_whole() {
        assert_eq!(tokens(r#"call post 1 "a b" title.2:"x \"y\"" "#), ["call", "post", "1", "\"a b\"", r#"title.2:"x \"y\"""#]);
        assert_eq!(unquote(r#""x \"y\" \\""#), Some(r#"x "y" \"#.to_string()));
        assert_eq!(unquote("plain"), None);
        let t = "q\"\\é";
        assert_eq!(unquote(&quote(t)).as_deref(), Some(t));
    }

    #[test]
    fn ticks_stay_on_ok_and_are_undone_on_a_refusal() {
        let mut w = World::new(0, 0);
        w.clock = 10;
        let mut exec: Exec<()> = Exec::default();
        let mut prov = Replay::default();
        let seen = exec.call(&mut w, &mut prov, |tx| -> Result<i64, Stop<()>> {
            tx.tick()?;
            tx.tick()?;
            Ok(tx.now)
        });
        assert_eq!((seen, w.clock), (Ok(12), 13), "two ticks, then the step to the next call");
        let r = exec.call(&mut w, &mut prov, |tx| -> Result<(), Stop<()>> {
            tx.tick()?;
            Err(Stop::Refused(()))
        });
        assert_eq!((r, w.clock), (Err(Stop::Refused(())), 14), "the tick undone, the call still takes one");
        w.clock = i64::MAX;
        let r = exec.call(&mut w, &mut prov, |tx| -> Result<(), Stop<()>> { tx.tick().map_err(Stop::Fault) });
        assert_eq!((r, w.clock), (Err(Stop::Fault(Fault::Overflow)), i64::MAX));
    }

    #[test]
    fn calls_nest_at_most_max_call_depth_deep() {
        fn down<E>(tx: &mut Tx<E>, n: u32) -> Result<(), Stop<()>> {
            tx.enter()?;
            if n > 0 {
                down(tx, n - 1)?;
            }
            tx.leave();
            Ok(())
        }
        let mut w = World::new(0, 0);
        let mut exec: Exec<()> = Exec::default();
        let mut prov = Replay::default();
        // n + 1 levels: 200 fits, 201 doesn't; returning frees the levels for the next call in the same body
        assert_eq!(exec.call(&mut w, &mut prov, |tx| down(tx, MAX_CALL_DEPTH - 1)), Ok(()));
        assert!(matches!(exec.call(&mut w, &mut prov, |tx| down(tx, MAX_CALL_DEPTH)), Err(Stop::Fault(_))));
        assert_eq!(exec.call(&mut w, &mut prov, |tx| { down(tx, 150)?; down(tx, 150) }), Ok(()));
    }

    #[test]
    fn world_reopen_keeps_the_id() {
        let mut w = World::new(1, 0);
        let a = w.open("A", 5, "u1");
        let b = w.open("B", 7, "u2");
        assert_eq!(w.open("A", 9, "u3"), a);
        assert_eq!((w.bal[a.0 as usize], w.bal[b.0 as usize], w.names.len(), w.amaps[0].len()), (9, 7, 2, 2));
        assert_eq!(w.owner_name(w.owner[a.0 as usize]), "u3");
    }
}
