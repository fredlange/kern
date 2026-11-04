"""Kern -> Rust. Turns a checked Kern module into a Rust crate that links with the runtime crate
kern-rt (runtime/).

    python -m kern.emit_rust MODULE.k [--out DIR] [--build]      (crates and the cargo target go in build/)

Each definition becomes one Rust function over a `Tx` (the runtime's call in progress). `need`s are
behaviour and are compiled; `keep`/`ensure`/`emit` are proven, so they compile to nothing. The
semantics are kern/run.py's (docs/research/RUNTIME_PLAN.md has the contract); validation/difftest.py checks the
generated program against run.py.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys

from .infer import uses as infer_uses
from .run import HANDLERS, assumed_op, channel_op, state_op
from .syntax import code, parse_module
from .wf import SIG, check_module

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNTIME = os.path.join(ROOT, "runtime")                 # the kern-rt crate
BUILD = os.path.join(ROOT, "build")                     # generated crates and the cargo target (ignored)
RUST_KEYWORDS = set("""as break const continue crate else enum extern false fn for if impl in let loop match mod
move mut pub ref return self static struct super trait true type unsafe use where while async await dyn
abstract become box do final macro override priv typeof unsized virtual yield try gen""".split())
RESERVED = RUST_KEYWORDS | {"tx", "rt", "w", "exec", "prov", "args"}
RUST_TYPE = {"Int": "i64", "Account": "Acct", "Owner": "Owner", "Bool": "bool"}


class Unsupported(Exception):
    """A construct the R1 generator does not compile (yet). Never silently mistranslated."""


def ident(name: str) -> str:
    name = name.replace(".", "_")
    return name + "_" if name in RESERVED else name


class Gen:
    def __init__(self, defs, modname):
        self.defs, self.modname = defs, modname
        self.refusals: dict[str, list] = {}       # ctor -> arg types, from `need ... else C args`
        self.events: dict[str, list] = {}         # ctor -> arg types, from constructor values
        self.channels = ["audit"] + list(getattr(defs, "channels", []))
        self.maps, slots = [], {"Account": 0, "Int": 0}
        for name, key in getattr(defs, "states", {}).items():
            self.maps.append((name, key, slots[key]))
            slots[key] += 1
        self.slot = {name: (key, s) for name, key, s in self.maps}

    # ---- constructors: one Rust variant per Kern constructor, with consistent argument types
    def ctor(self, table, name, types, where):
        if any(t not in RUST_TYPE for t in types):
            raise Unsupported(f"{where}: constructor {name} with a {'/'.join(types)} argument")
        if table.setdefault(name, types) != types:
            raise Unsupported(f"{where}: constructor {name} used with {table[name]} and with {types}")

    # ---- expressions: (rust code, Kern type)
    def expr(self, e, sc):
        k = e[0]
        if k == "int":
            return f"{e[1]}i64", "Int"
        if k == "bin":
            op = e[1]
            if op == "when":
                raise Unsupported("`when` outside a spec")
            l, _ = self.expr(e[2], sc)
            r, rt_ = self.expr(e[3], sc)
            if op in ("+", "-", "*"):
                return f"rt::{ {'+': 'add', '-': 'sub', '*': 'mul'}[op] }({l}, {r})?", "Int"
            if op == "/":
                return f"rt::div({l}, {r})", "Int"
            if op in ("and", "or"):
                return f"({l} {'&&' if op == 'and' else '||'} {r})", "Bool"
            return f"({l} {'==' if op == '=' else op} {r})", "Bool"
        if k == "name":
            name, primed = e[1], e[2]
            if primed:
                raise Unsupported(f"after-state read {name}' outside a spec")
            if name in sc:
                return ident(name) + (".clone()" if sc[name] == "Event" else ""), sc[name]
            return self.apply(name, [], sc)
        if k == "app":
            (name, primed), args = e[1], e[2]
            if primed:
                raise Unsupported(f"after-state read {name}' outside a spec")
            return self.apply(name, args, sc)
        raise Unsupported(f"expression {code(e)}")

    @staticmethod
    def hoist(fmt, vals):
        """Arguments that call into the transaction are evaluated into temporaries first (left to
        right, as run.py does), so no argument borrows `tx` while the call holds it."""
        codes = [c for c, _ in vals]
        if not any("tx" in c for c in codes):
            return fmt.format(*codes)
        temps = [f"__a{i}" for i in range(len(codes))]
        lets = " ".join(f"let {t} = {c};" for t, c in zip(temps, codes))
        return f"{{ {lets} {fmt.format(*temps)} }}"

    def apply(self, name, args, sc):
        vals = [self.expr(a, sc) for a in args]
        types = [t for _, t in vals]
        if name in HANDLERS:
            fmt = {
                "ledger.balance": "tx.balance({})",
                "ledger.owner": "tx.owner({})",
                "ledger.total": "tx.total()?",
                "ledger.debit": "tx.debit({}, {})?",
                "ledger.credit": "tx.credit({}, {})?",
                "clock.now": "tx.now",
                "audit.emit": "tx.emit(0, {})",
            }[name]
            return self.hoist(fmt, vals), SIG[name][1]
        if assumed_op(self.defs, name):
            ph = ", ".join("{}" for _ in vals)
            return self.hoist(f"h__{ident(name)}(tx{', ' if vals else ''}{ph})?", vals), "Int"
        ch = channel_op(self.defs, name)
        if ch:
            return self.hoist(f"tx.emit({self.channels.index(ch)}, {{}})", vals), "Unit"
        st = state_op(self.defs, name)
        if st:
            x, op, key = st
            pre = "a" if key == "Account" else "i"
            slot = self.slot[x][1]
            if op == "get":
                return self.hoist(f"tx.{pre}get({slot}, {{}})", vals), "Int"
            return self.hoist(f"tx.{pre}set({slot}, {{}}, {{}})", vals), "Unit"
        if name in self.defs:
            d = self.defs[name]
            ph = ", ".join("{}" for _ in vals)
            return self.hoist(f"{ident(name)}(tx{', ' if vals else ''}{ph})?", vals), ("Int" if d.ret else "Unit")
        if name == "distinct":
            temps = [f"__d{i}" for i in range(len(vals))]
            lets = " ".join(f"let {t} = {c};" for t, (c, _) in zip(temps, vals))
            pairs = " && ".join(f"{a} != {b}" for i, a in enumerate(temps) for b in temps[i + 1:])
            return f"{{ {lets} {pairs} }}", "Bool"
        if name[:1].isupper():
            self.ctor(self.events, name, types, f"event {name}")
            if not vals:
                return f"Event::{name}", "Event"
            return f"Event::{name}({', '.join(c for c, _ in vals)})", "Event"
        raise Unsupported(f"unknown name {name}")

    # ---- statements
    def need(self, s, sc, ind, d):
        if s[0] != "need":
            raise Unsupported(f"{d.name}: spec need {s[0]}")
        cond, _ = self.expr(s[1], sc)
        vals = [self.expr(a, sc) for a in s[3]]
        self.ctor(self.refusals, s[2], [t for _, t in vals], f"{d.name}: refusal {s[2]}")
        payload = f"({', '.join(c for c, _ in vals)})" if vals else ""
        return [f"{ind}if !({cond}) {{ return Err(Stop::Refused(Refusal::{s[2]}{payload})); }}"]

    def block(self, stmts, sc, ind, d):
        out = []
        for s in stmts:
            k = s[0]
            if k == "need":
                out += self.need(s, sc, ind, d)
            elif k == "let":
                c, t = self.expr(s[2], sc)
                if s[1] == "result" and d.ret:
                    out.append(f"{ind}result = {c};")
                else:
                    if t == "Unit":
                        raise Unsupported(f"{d.name}: let of a Unit value")
                    out.append(f"{ind}let {ident(s[1])} = {c};")
                    sc[s[1]] = t
            elif k == "if":
                c, _ = self.expr(s[1], sc)
                out.append(f"{ind}if ({c}) {{")
                out += self.block(s[2], dict(sc), ind + "    ", d)
                if s[3]:
                    out.append(f"{ind}}} else {{")
                    out += self.block(s[3], dict(sc), ind + "    ", d)
                out.append(f"{ind}}}")
            elif k == "for":                    # bounds evaluated once; invariants are proven
                lo, _ = self.expr(s[2], sc)
                hi, _ = self.expr(s[3], sc)
                out.append(f"{ind}for {ident(s[1])} in ({lo})..({hi}) {{")
                out += self.block(s[5], dict(sc, **{s[1]: "Int"}), ind + "    ", d)
                out.append(f"{ind}}}")
            elif k == "do":
                c, _ = self.expr(s[1], sc)
                out.append(f"{ind}{c};")
            else:
                raise Unsupported(f"{d.name}: statement {k}")
        return out

    def definition(self, d):
        sc = {p: t for p, t in d.params}
        params = "".join(f", {ident(p)}: {RUST_TYPE[t]}" for p, t in d.params)
        ret = "i64" if d.ret else "()"
        out = [f"/// {d.src_lines['header'][0]}", "#[inline]",
               f"pub fn {ident(d.name)}(tx: &mut Tx<Event>{params}) -> R<{ret}> {{"]
        if d.ret:
            out.append("    let mut result: i64;")
        out += self.block(list(d.needs) + list(d.body), sc, "    ", d)
        out.append("    Ok(result)" if d.ret else "    Ok(())")
        out.append("}")
        return out

    def handler(self, h):
        """An assumed handler: check its needs, ask the provider, check its ensures (run.py does the
        same; a broken assumption is a fault, never a refusal)."""
        sc = {p: t for p, t in h.params}
        params = "".join(f", {ident(p)}: {RUST_TYPE[t]}" for p, t in h.params)
        vals = ", ".join(f"Val::Int({ident(p)})" if t == "Int" else f"Val::Acct({ident(p)})" for p, t in h.params)
        out = [f"/// {h.src_lines['header'][0]}", f"fn h__{ident(h.name)}(tx: &mut Tx<Event>{params}) -> R<i64> {{"]
        for n in h.needs:
            c, _ = self.expr(n[1], sc)
            msg = json.dumps(f"need of {h.name} not met: {code(n[1])}")
            out.append(f"    if !({c}) {{ return Err(Fault::Error({msg}.into()).into()); }}")
        out.append(f"    let result = tx.provide({json.dumps(h.name)}, &[{vals}])?;")
        sc["result"] = "Int"
        for e in h.ensures:
            if e[0] == "bin" and e[1] == "when":
                cond, _ = self.expr(e[3], sc)
                body, _ = self.expr(e[2], sc)
                c = f"(!{cond} || {body})"
            else:
                c, _ = self.expr(e, sc)
            msg = json.dumps(f"the provider of {h.name} broke its assumed spec: {code(e)}")
            out.append(f"    if !({c}) {{ return Err(Fault::Error(format!(\"{{}} with result {{}}\", {msg}, result)).into()); }}")
        out.append(f"    tx.record({json.dumps(h.name)}, &[{vals}], result);")
        out.append("    Ok(result)")
        out.append("}")
        return out

    # ---- the module
    def show_arm(self, enum, name, types):
        if not types:
            return f"            {enum}::{name} => {json.dumps(name)}.to_string(),"
        vs = [f"x{i}" for i in range(len(types))]
        parts = []
        for v, t in zip(vs, types):
            parts.append({"Int": f"{v}.to_string()", "Account": f"w.name(*{v}).to_string()",
                          "Owner": f"w.owner_name(*{v}).to_string()", "Bool": f"rt::show_bool(*{v}).to_string()"}[t])
        return (f"            {enum}::{name}({', '.join(vs)}) => [{json.dumps(name)}.to_string(), "
                f"{', '.join(parts)}].join(\" \"),")

    def enum(self, name, table):
        if not table:
            return [f"#[derive(Clone, Debug, PartialEq)]", f"pub enum {name} {{}}"]
        out = ["#[derive(Clone, Debug, PartialEq)]", f"pub enum {name} {{"]
        for c, types in table.items():
            out.append(f"    {c}({', '.join(RUST_TYPE[t] for t in types)})," if types else f"    {c},")
        out.append("}")
        return out

    def module(self):
        body = []
        for h in getattr(self.defs, "handlers", {}).values():
            body += self.handler(h) + [""]
        for d in self.defs.values():
            body += self.definition(d) + [""]
        out = [
            f"// GENERATED by kern/emit_rust.py from {self.modname}.k. Do not edit: edit the .k file and rebuild.",
            "// Semantics: kern/run.py (the reference); contract and divergences: docs/research/RUNTIME_PLAN.md.",
            "#![allow(unused_imports, dead_code, unused_variables, unused_mut, unused_parens, unused_braces, unused_assignments, non_snake_case, clippy::all)]",
            "use kern_rt as rt;",
            "use kern_rt::{Acct, DefInfo, Exec, Fault, KeyType, MapInfo, Owner, Provider, Stop, Tx, Val, World};",
            "",
        ]
        out += self.enum("Refusal", self.refusals) + [""]
        out += self.enum("Event", self.events) + [""]
        out += ["type R<T> = Result<T, Stop<Refusal>>;", ""]
        out += body
        defs = list(self.defs.values())
        out += ["pub struct M;", "", "impl rt::Module for M {", "    type Refusal = Refusal;", "    type Event = Event;",
                f"    const NAME: &'static str = {json.dumps(self.modname)};", "    const DEFS: &'static [DefInfo] = &["]
        for d in defs:
            ps = ", ".join(f"({json.dumps(p)}, KeyType::{'Account' if t == 'Account' else 'Int'})" for p, t in d.params)
            us = ", ".join(json.dumps(u) for u in sorted(infer_uses(self.defs, d)))
            out.append(f"        DefInfo {{ name: {json.dumps(d.name)}, hash: {json.dumps(d.hash)}, params: &[{ps}], "
                       f"ret: {'true' if d.ret else 'false'}, uses: &[{us}] }},")
        out.append("    ];")
        out.append("    const MAPS: &'static [MapInfo] = &[" + ", ".join(
            f"MapInfo {{ name: {json.dumps(n)}, key: KeyType::{k}, slot: {s} }}" for n, k, s in self.maps) + "];")
        out.append("    const CHANNELS: &'static [&'static str] = &[" + ", ".join(json.dumps(c) for c in self.channels) + "];")
        out += ["    fn call(exec: &mut Exec<Event>, w: &mut World, prov: &mut dyn Provider, def: usize, args: &[Val]) -> R<Option<i64>> {",
                "        match def {"]
        for i, d in enumerate(defs):
            conv = ", ".join(f"rt::{'acct' if t == 'Account' else 'int'}(args[{j}])" for j, (_, t) in enumerate(d.params))
            wrap = ".map(Some)" if d.ret else ".map(|_| None)"
            out.append(f"            {i} => exec.call(w, prov, |tx| {ident(d.name)}(tx{', ' if conv else ''}{conv}){wrap}),")
        out += ["            _ => unreachable!(\"the driver checked the definition\"),", "        }", "    }",
                "    fn show_refusal(r: &Refusal, w: &World) -> String {", "        match r {"]
        out += [self.show_arm("Refusal", c, t) for c, t in self.refusals.items()]
        if not self.refusals:
            out.append("            _ => unreachable!(),")
        out += ["        }", "    }", "    fn show_event(e: &Event, w: &World) -> String {", "        match e {"]
        out += [self.show_arm("Event", c, t) for c, t in self.events.items()]
        if not self.events:
            out.append("            _ => unreachable!(),")
        out += ["        }", "    }", "}"]
        return "\n".join(out) + "\n"


def crate_name(path):
    return re.sub(r"\W", "_", os.path.splitext(os.path.basename(path))[0])


def emit(path, out_dir=None):
    """Generate the crate for module `path`; returns its directory. Raises Unsupported."""
    name = crate_name(path)
    defs = check_module(parse_module(open(path).read()))
    src = Gen(defs, name).module()
    out_dir = out_dir or os.path.join(BUILD, name)
    os.makedirs(os.path.join(out_dir, "src"), exist_ok=True)
    files = {
        "Cargo.toml": f"""# GENERATED by kern/emit_rust.py
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
kern-rt = {{ path = "{RUNTIME}" }}

[profile.release]
opt-level = 3
codegen-units = 1

[workspace]
""",
        "src/main.rs": "// GENERATED by kern/emit_rust.py\nmod module;\n\nfn main() {\n    kern_rt::driver::main::<module::M>()\n}\n",
        "src/module.rs": src,
    }
    for rel, text in files.items():
        p = os.path.join(out_dir, rel)
        if not os.path.exists(p) or open(p).read() != text:      # keep mtimes stable: cargo rebuilds less
            open(p, "w").write(text)
    return out_dir


def build(crate_dir):
    """cargo build --release; returns the binary path. Warnings are errors (scorecard C1)."""
    env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(BUILD, "target"), RUSTFLAGS="-D warnings")
    r = subprocess.run(["cargo", "build", "--release", "-q"], cwd=crate_dir, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"cargo build failed in {crate_dir}:\n{r.stderr}")
    return os.path.join(BUILD, "target", "release", os.path.basename(crate_dir))


def main(argv):
    import argparse
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("module")
    ap.add_argument("--out")
    ap.add_argument("--build", action="store_true")
    a = ap.parse_args(argv)
    try:
        d = emit(a.module, a.out)
    except Unsupported as e:
        print(f"unsupported: {e}", file=sys.stderr)
        return 1
    print(d)
    if a.build:
        print(build(d))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
