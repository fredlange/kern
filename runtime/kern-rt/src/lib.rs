//! kern-rt: the hand-written half of every compiled Kern program.
//!
//! `runtime/emit_rust.py` turns a Kern module into Rust functions that run against a [`Tx`]; this
//! crate owns everything else: the world, atomic calls (undo log), checked `Int` arithmetic, the
//! clock, assumed-handler providers with recording, and the driver. The semantics are those of the
//! reference interpreter `kern/run.py`; docs/RUNTIME_PLAN.md lists the contract and the one
//! deliberate divergence (Int is i64; leaving it is a fault).
use std::collections::{HashMap, VecDeque};

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val {
    Int(i64),
    Acct(Acct),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    Int,
    Account,
}

/// What the compiler knows about a definition.
pub struct DefInfo {
    pub name: &'static str,
    pub hash: &'static str,
    pub params: &'static [(&'static str, KeyType)],
    pub ret: bool,
    pub uses: &'static [&'static str],
}

/// A declared `state NAME KeyType`; `slot` indexes the account maps or the Int maps.
pub struct MapInfo {
    pub name: &'static str,
    pub key: KeyType,
    pub slot: usize,
}

// ---- the world

pub struct World {
    pub bal: Vec<i64>,
    pub owner: Vec<Owner>,
    pub names: Vec<String>,
    pub index: HashMap<String, u32>,
    pub owners: Vec<String>,
    pub owner_index: HashMap<String, u32>,
    /// `state X Account`: dense, one entry per account; missing = 0
    pub amaps: Vec<Vec<i64>>,
    /// `state X Int`: sparse; missing = 0
    pub imaps: Vec<HashMap<i64, i64>>,
    pub clock: i64,
}

impl World {
    pub fn new(n_amaps: usize, n_imaps: usize) -> Self {
        World {
            bal: Vec::new(),
            owner: Vec::new(),
            names: Vec::new(),
            index: HashMap::new(),
            owners: Vec::new(),
            owner_index: HashMap::new(),
            amaps: vec![Vec::new(); n_amaps],
            imaps: vec![HashMap::new(); n_imaps],
            clock: 0,
        }
    }

    pub fn acct(&self, name: &str) -> Option<Acct> {
        self.index.get(name).map(|&i| Acct(i))
    }

    pub fn name(&self, a: Acct) -> &str {
        &self.names[a.0 as usize]
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
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), id);
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
}

/// Reusable per-thread buffers, so a call allocates nothing in the steady state.
pub struct Exec<E> {
    undo: Vec<Undo>,
    /// (channel index, event) for the last call; channel 0 is `audit`
    pub events: Vec<(u16, E)>,
    pub obs: Vec<Obs>,
}

impl<E> Default for Exec<E> {
    fn default() -> Self {
        Exec { undo: Vec::new(), events: Vec::new(), obs: Vec::new() }
    }
}

/// One call in progress. Writes go straight to the world and are recorded for rollback.
pub struct Tx<'a, E> {
    w: &'a mut World,
    undo: &'a mut Vec<Undo>,
    events: &'a mut Vec<(u16, E)>,
    obs: &'a mut Vec<Obs>,
    prov: &'a mut dyn Provider,
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
        let out = {
            let mut tx = Tx { w: &mut *w, undo: &mut self.undo, events: &mut self.events, obs: &mut self.obs, prov, now };
            body(&mut tx)
        };
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
        let n_a = Self::MAPS.iter().filter(|m| m.key == KeyType::Account).count();
        World::new(n_a, Self::MAPS.len() - n_a)
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
pub fn acct(v: Val) -> Acct {
    match v {
        Val::Acct(a) => a,
        Val::Int(_) => unreachable!("checked at the boundary"),
    }
}
#[inline(always)]
pub fn int(v: Val) -> i64 {
    match v {
        Val::Int(n) => n,
        Val::Acct(_) => unreachable!("checked at the boundary"),
    }
}
