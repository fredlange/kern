"""Lexer and parser for the Kern prototype.

A module is a sequence of definitions:

    name param:Type ... [-> Type]      header (column 0)
      need EXPR else Ctor ARGS          behavioural guard: fails with Ctor if EXPR is false
      keep EXPR                         EXPR has the same value before and after (on success)
      ensure EXPR                       postcondition; primed reads (x') see the after-state
      emit Ctor ARGS                    the call appends exactly these audit events, in order
                                        (all emit clauses together); `_` matches any value
      eg WORLD | CALL -> OUTCOME [| WORLD]   executable example
    body
      x = EXPR
      need EXPR else Ctor ARGS
      EXPR                              statement (call for its effect)

Expressions use juxtaposition for application: `ledger.balance from`. Arguments are
atoms; anything compound is parenthesised: `Insufficient (amount - have)`.

AST nodes are plain tuples so mutation and hashing stay trivial:
    ('int', n) ('name', s, primed) ('bin', op, l, r) ('app', head_name, [args])
    statements: ('let', x, e) ('need', cond, ctor, [args]) ('do', e)
"""
from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass, field

TOKEN = re.compile(r"\s*(?:(\d+)|([A-Za-z_][A-Za-z0-9_.]*'?)|(!=|>=|<=|->|[=<>+\-*/()]))")
CMP = {"=", "!=", ">=", "<=", ">", "<"}
KEYWORDS = ("and", "or", "else", "when")


class ParseError(Exception):
    def __init__(self, msg, line_no=None):
        super().__init__(f"line {line_no}: {msg}" if line_no else msg)


def lex(s: str, line_no=None):
    toks, pos = [], 0
    s = s.rstrip()
    while pos < len(s):
        m = TOKEN.match(s, pos)
        if not m or m.end() == pos:
            raise ParseError(f"unexpected character {s[pos:].strip()[:1]!r} in {s.strip()!r}", line_no)
        if m.group(1):
            toks.append(("int", int(m.group(1))))
        elif m.group(2):
            toks.append(("name", m.group(2)))
        else:
            toks.append(("op", m.group(3)))
        pos = m.end()
    return toks


class ExprParser:
    def __init__(self, toks, line_no=None):
        self.t, self.i, self.line_no = toks, 0, line_no

    def peek(self):
        return self.t[self.i] if self.i < len(self.t) else (None, None)

    def take(self):
        tok = self.peek()
        self.i += 1
        return tok

    def expect_end(self):
        if self.i != len(self.t):
            raise ParseError(f"unexpected {self.peek()[1]!r}", self.line_no)

    def expr(self):
        """`E when C` (only at the top of an ensure; the checker enforces that) over or over and
        (both left-associative, `and` binding tighter) over comparisons over sums."""
        left = self.or_()
        if self.peek() == ("name", "when"):
            self.take()
            return ("bin", "when", left, self.or_())
        return left

    def or_(self):
        left = self.and_()
        while self.peek() == ("name", "or"):
            self.take()
            left = ("bin", "or", left, self.and_())
        return left

    def and_(self):
        left = self.cmp()
        while self.peek() == ("name", "and"):
            self.take()
            left = ("bin", "and", left, self.cmp())
        return left

    def cmp(self):
        left = self.sum()
        kind, val = self.peek()
        if kind == "op" and val in CMP:
            self.take()
            return ("bin", val, left, self.sum())
        return left

    def sum(self):
        left = self.term()
        while self.peek() in (("op", "+"), ("op", "-")):
            op = self.take()[1]
            left = ("bin", op, left, self.term())
        return left

    def term(self):
        left = self.app()
        while self.peek() in (("op", "*"), ("op", "/")):
            op = self.take()[1]
            left = ("bin", op, left, self.app())
        return left

    def app(self):
        kind, val = self.peek()
        if kind == "name" and val not in KEYWORDS:
            self.take()
            args = []
            while self.starts_atom():
                args.append(self.atom())
            name, primed = (val[:-1], True) if val.endswith("'") else (val, False)
            if args:
                return ("app", (name, primed), args)
            return ("name", name, primed)
        return self.atom()

    def starts_atom(self):
        kind, val = self.peek()
        return kind == "int" or (kind == "name" and val not in KEYWORDS) or (kind, val) == ("op", "(")

    def atom(self):
        kind, val = self.take()
        if kind == "int":
            return ("int", val)
        if kind == "name":
            return ("name", val[:-1], True) if val.endswith("'") else ("name", val, False)
        if (kind, val) == ("op", "("):
            e = self.expr()
            if self.take() != ("op", ")"):
                raise ParseError("missing ')'", self.line_no)
            return e
        raise ParseError(f"expected a value, got {val!r}", self.line_no)


def parse_expr(src: str, line_no=None):
    p = ExprParser(lex(src, line_no), line_no)
    e = p.expr()
    p.expect_end()
    return e


_ATOM = r"(\([^()]*\)|\S+)"                      # a bound: a name, a number, or (parenthesised)
FOR_SUFFIX = re.compile(rf"^(.*) for ([a-z]\w*) from {_ATOM} to {_ATOM}$")
FOR_LOOP = re.compile(rf"^for ([a-z]\w*) from {_ATOM} to {_ATOM}$")


def parse_clause(src: str, line_no):
    """An ensure / loop-invariant clause: `E [when C] [for j from A to B]` -> expr or
    ("all", j, A, B, expr): E (when C) holds for every j in [A, B). Bounds are atoms, because
    `from` and `to` are common parameter names."""
    m = FOR_SUFFIX.match(src.strip())
    if m:
        return ("all", m.group(2), parse_expr(m.group(3), line_no), parse_expr(m.group(4), line_no),
                parse_expr(m.group(1), line_no))
    return parse_expr(src, line_no)


def parse_emit(src: str, line_no):
    """`emit Ctor ARGS [when COND]` -> (Ctor, args) or (Ctor, args, cond)."""
    if " when " in src:
        ev, cond = src.split(" when ", 1)
        return (*parse_ctor(ev, line_no, "emit"), parse_expr(cond, line_no))
    return parse_ctor(src, line_no, "emit")


def emit_cond(ev):
    return ev[2] if len(ev) > 2 else None


def parse_ctor(src: str, line_no, what):
    f = parse_expr(src, line_no)
    if f[0] == "name" and not f[2]:
        ctor, args = f[1], []
    elif f[0] == "app" and not f[1][1]:
        ctor, args = f[1][0], f[2]
    else:
        raise ParseError(f"{what} must name a constructor", line_no)
    if not ctor[0].isupper():
        raise ParseError(f"{ctor!r} must be Capitalised", line_no)
    return ctor, args


def parse_need(src: str, line_no):
    if " else " not in src:
        raise ParseError("need must say what it fails with: need COND else Ctor", line_no)
    cond, fail = src.rsplit(" else ", 1)
    if FOR_SUFFIX.match(cond.strip()):
        raise ParseError("`for j from A to B` is only allowed at the end of an `ensure` or a loop `invariant`", line_no)
    f = parse_expr(fail, line_no)
    if f[0] == "name":
        ctor, args = f[1], []
    elif f[0] == "app":
        ctor, args = f[1][0], f[2]
    else:
        raise ParseError("else must name a failure constructor", line_no)
    if not ctor[0].isupper():
        raise ParseError(f"failure {ctor!r} must be Capitalised", line_no)
    return ("need", parse_expr(cond, line_no), ctor, args)


class Module(dict):
    """Definitions by name, plus module-level declarations: `state NAME KeyType` (a map from keys
    to Int, default 0, read with NAME.get k and written with NAME.set k v) and
    `invariant x:Type COND` (holds for every key before and after every call)."""

    def __init__(self, *a, states=None, invariants=None, channels=None, handlers=None, **kw):
        super().__init__(*a, **kw)
        self.states = dict(states or {})
        self.invariants = list(invariants or [])
        self.channels = list(channels or [])         # `channel NAME`: output delivered after commit
        self.handlers = dict(handlers or {})         # `handler a.b params -> Int`: assumed spec, no body

    def with_def(self, d):
        return Module(self, states=self.states, invariants=self.invariants, channels=self.channels,
                      handlers=self.handlers, **{d.name: d})


def with_def(defs, d):
    """defs with d replacing the definition of the same name, keeping module declarations."""
    return defs.with_def(d) if isinstance(defs, Module) else dict(defs, **{d.name: d})


@dataclass
class Example:
    world: dict            # account -> (balance, owner)
    call: tuple            # (def name, [arg tokens as str|int])
    outcome: tuple         # ('ok',) or (Ctor, [ints])
    after: dict            # account -> balance
    src: str
    now: int = 0           # the call's time (clock.now), written `now:N` in the world
    maps: dict = field(default_factory=dict)        # state -> {key: value}, written `hold.7:100`
    after_maps: dict = field(default_factory=dict)


@dataclass
class Def:
    name: str
    params: list           # [(name, type)]
    ret: str | None
    needs: list = field(default_factory=list)
    keeps: list = field(default_factory=list)
    ensures: list = field(default_factory=list)
    emits: list = field(default_factory=list)     # [(Ctor, [arg exprs])]; empty = audit unconstrained
    complete: bool = False                         # `ok otherwise`: the spec needs are every refusal
    ref_origin: dict = field(default_factory=dict)  # (kind, index) -> callee, for `need as` / `emit as`
    emit_channel: dict = field(default_factory=dict)  # emit index -> channel; missing = `audit`
    examples: list = field(default_factory=list)
    body: list = field(default_factory=list)
    src_lines: dict = field(default_factory=dict)   # section -> [source lines]

    @property
    def hash(self) -> str:
        parts = (self.params, self.ret, self.needs, self.keeps, self.ensures, self.body)
        if self.emits:                      # only when present, so older definitions keep their hash
            parts += (self.emits,)
        if self.complete:
            parts += ("ok otherwise",)
        if self.emit_channel:                # only when present, so audit-only definitions keep their hash
            parts += (sorted(self.emit_channel.items()),)
        canon = repr(parts)
        return hashlib.sha256(canon.encode()).hexdigest()[:8]

    @property
    def content_hash(self) -> str:
        """Everything, examples included: the key for a content-addressed store. `hash` leaves
        examples out on purpose (they are evidence, not semantics, so they don't stale a why)."""
        canon = repr((self.hash, [e.src for e in self.examples]))
        return hashlib.sha256(canon.encode()).hexdigest()[:8]


WORLD_ITEM = re.compile(r"^([A-Z]\w*):(\d+)(?:@(\w+))?$")


NOW_ITEM = re.compile(r"^now:(\d+)$")
MAP_ITEM = re.compile(r"^([a-z]\w*)\.([A-Z]\w*|-?\d+):(-?\d+)$")


def split_maps(src):
    """Take `state.key:value` items out of a world: (rest, {state: {key: value}})."""
    items, maps = [], {}
    for item in src.split():
        m = MAP_ITEM.match(item)
        if m:
            key = m.group(2) if m.group(2)[0].isalpha() else int(m.group(2))
            maps.setdefault(m.group(1), {})[key] = int(m.group(3))
        else:
            items.append(item)
    return " ".join(items), maps


def split_now(src):
    """Take an optional `now:N` item out of a world: (rest, N)."""
    items, now = [], 0
    for item in src.split():
        m = NOW_ITEM.match(item)
        if m:
            now = int(m.group(1))
        else:
            items.append(item)
    return " ".join(items), now


def parse_world(src, line_no, with_owner=True):
    w = {}
    for item in src.split():
        m = WORLD_ITEM.match(item)
        if not m or (with_owner and not m.group(3)):
            raise ParseError(f"bad world item {item!r} (want A:500@u1)", line_no)
        w[m.group(1)] = (int(m.group(2)), m.group(3)) if with_owner else int(m.group(2))
    return w


def parse_example(src, line_no):
    parts = [p.strip() for p in src.split("|")]
    if len(parts) not in (2, 3) or "->" not in parts[1]:
        raise ParseError("eg WORLD | call args -> outcome [| AFTER]", line_no)
    world_src, now = split_now(parts[0])
    world_src, maps = split_maps(world_src)
    world = parse_world(world_src, line_no)
    call_src, out_src = [s.strip() for s in parts[1].split("->")]
    call = call_src.split()
    args = [int(a) if a.lstrip("-").isdigit() else a for a in call[1:]]
    out = out_src.split()
    outcome = ("ok",) if out == ["ok"] else (out[0], [int(x) for x in out[1:]])
    after_src, after_maps = split_maps(parts[2]) if len(parts) == 3 else ("", {})
    after = parse_world(after_src, line_no, with_owner=False)
    return Example(world, (call[0], args), outcome, after, src, now, maps, after_maps)


HEADER = re.compile(r"^([a-z]\w*)((?:\s+\w+:\w+)*)\s*(?:->\s*(\w+))?\s*$")


STATE = re.compile(r"^state ([a-z]\w*) (Int|Account)$")
CHANNEL = re.compile(r"^channel ([a-z]\w*)$")
HANDLER = re.compile(r"^handler ([a-z]\w*(?:\.[a-z]\w*)+)((?:\s+\w+:\w+)*)\s*(?:->\s*(\w+))?\s*$")
INVARIANT = re.compile(r"^invariant ([a-z]\w*):(Int|Account) (.+)$")


def parse_module(text: str) -> dict:
    defs, cur, in_body = Module(), None, False
    for no, raw in enumerate(text.splitlines(), 1):
        if not raw.strip():
            continue
        line = raw.strip()
        if not raw[0].isspace():
            if line.startswith("handler "):           # an observation with an assumed spec
                m = HANDLER.match(line)
                if not m:
                    raise ParseError("want `handler NAME.op p:Type ... -> Int`", no)
                _finish(cur)
                params = [tuple(p.split(":")) for p in m.group(2).split()]
                cur = Def(m.group(1), params, m.group(3))
                cur.src_lines = {"header": [line], "spec": [], "body": []}
                cur._raw, cur.assumed = [], True
                defs.handlers[cur.name] = cur
                in_body = False
                continue
            if line.startswith(("state ", "invariant ", "channel ")):
                m, inv, ch = STATE.match(line), INVARIANT.match(line), CHANNEL.match(line)
                if ch:
                    if ch.group(1) in ("audit", "as"):
                        raise ParseError(f"`{ch.group(1)}` can't be declared as a channel", no)
                    defs.channels.append(ch.group(1))
                elif m:
                    defs.states[m.group(1)] = m.group(2)
                elif inv:
                    defs.invariants.append((inv.group(1), inv.group(2), parse_expr(inv.group(3), no)))
                else:
                    raise ParseError("want `state NAME Int|Account`, `invariant x:Type COND` or `channel NAME`", no)
                _finish(cur)
                cur, in_body = None, False
                continue
            if line == "body":
                if cur is None:
                    raise ParseError("body without a definition", no)
                if getattr(cur, "assumed", False):
                    raise ParseError(f"handler {cur.name} has an assumed spec and no body", no)
                in_body = True
                continue
            m = HEADER.match(line)
            if not m:
                raise ParseError(f"bad definition header {line!r}", no)
            params = [tuple(p.split(":")) for p in m.group(2).split()]
            _finish(cur)
            cur = Def(m.group(1), params, m.group(3))
            cur.src_lines = {"header": [line], "spec": [], "body": []}
            cur._raw = []
            defs[cur.name] = cur
            in_body = False
            continue
        if cur is None:
            raise ParseError("indented line outside a definition", no)
        word, _, rest = line.partition(" ")
        if in_body:
            indent = len(raw) - len(raw.lstrip())
            cur._raw.append((indent, line, no))
            continue
        cur.src_lines["spec"].append(line)
        if word == "need" and rest.startswith("as "):       # `need as callee ARGS`: its spec needs
            cur.needs.append(("need_as", *_ref(rest[3:], no)))
        elif word == "emit" and rest.startswith("as "):     # `emit as callee ARGS [when C]`
            ref, _, cond = rest[3:].partition(" when ")
            cur.emits.append(("emit_as", *_ref(ref, no), parse_expr(cond, no) if cond else None))
        elif word == "need":
            cur.needs.append(parse_need(rest, no))
        elif word == "keep":
            cur.keeps.append(parse_expr(rest, no))
        elif word == "ensure":
            cur.ensures.append(parse_clause(rest, no))
        elif word == "emit":
            first, _, after = rest.partition(" ")
            if first[:1].islower() and after:            # `emit console Line x`: a named channel
                cur.emit_channel[len(cur.emits)] = first
                rest = after
            cur.emits.append(parse_emit(rest, no))
        elif word == "eg":
            cur.examples.append(parse_example(rest, no))
        elif word == "ok":
            if rest.split() != ["otherwise"]:
                raise ParseError("the clause is `ok otherwise` (succeeds whenever the needs hold)", no)
            if cur.complete:
                raise ParseError("`ok otherwise` appears twice", no)
            cur.complete = True
        else:
            raise ParseError(f"unknown clause {word!r} (need/keep/ensure/emit/ok/eg, then body)", no)
    _finish(cur)
    _expand_refs(defs)
    return defs


def _ref(src, no):
    e = parse_expr(src, no)
    if e[0] == "app" and not e[1][1]:
        return e[1][0], e[2], no
    if e[0] == "name" and not e[2]:
        return e[1], [], no
    raise ParseError("`as` must name a definition and its arguments", no)


def _subst(e, m):
    if e[0] == "name":
        return m.get(e[1], e) if not e[2] else e
    if e[0] == "bin":
        return ("bin", e[1], _subst(e[2], m), _subst(e[3], m))
    if e[0] == "app":
        return ("app", e[1], [_subst(a, m) for a in e[2]])
    return e


def _expand_refs(defs):
    """Replace `need as C args` / `emit as C args` by C's spec needs / emits with C's parameters
    replaced by args. The reference is written in the spec, so it follows the spec's arguments, not
    the body's; the result is checked like any written clause. d.ref_origin marks which need or emit
    came from which reference (for the page)."""
    done, busy = set(), set()

    def expand(d):
        if d.name in done:
            return
        if d.name in busy:
            raise ParseError(f"`as` references loop through {d.name}")
        busy.add(d.name)
        needs, emits, d.ref_origin = [], [], {}
        own_channel, d.emit_channel = d.emit_channel, {}
        for n in d.needs:
            if n[0] != "need_as":
                needs.append(n)
                continue
            callee = _callee(defs, n)
            expand(callee)
            m = _bind(callee, n)
            for c in callee.needs:
                d.ref_origin[("need", len(needs))] = n[1]
                needs.append(("need", _subst(c[1], m), c[2], [_subst(a, m) for a in c[3]]))
        for i, ev in enumerate(d.emits):
            if ev[0] != "emit_as":
                if i in own_channel:
                    d.emit_channel[len(emits)] = own_channel[i]
                emits.append(ev)
                continue
            callee = _callee(defs, ev)
            expand(callee)
            m, extra = _bind(callee, ev), ev[4]
            for j, (ctor, args, *cond) in enumerate(callee.emits):
                c = _subst(cond[0], m) if cond else None
                c = ("bin", "and", extra, c) if extra and c else (extra or c)
                d.ref_origin[("emit", len(emits))] = ev[1]
                if j in callee.emit_channel:
                    d.emit_channel[len(emits)] = callee.emit_channel[j]
                emits.append((ctor, [_subst(a, m) for a in args], c) if c else (ctor, [_subst(a, m) for a in args]))
        d.needs, d.emits = needs, emits
        busy.discard(d.name)
        done.add(d.name)

    for d in list(defs.values()):
        expand(d)


def _callee(defs, ref):
    if ref[1] not in defs:
        raise ParseError(f"`as {ref[1]}`: no definition named {ref[1]!r}", ref[3])
    return defs[ref[1]]


def _bind(callee, ref):
    if len(ref[2]) != len(callee.params):
        raise ParseError(f"`as {ref[1]}` takes {len(callee.params)} argument(s), got {len(ref[2])}", ref[3])
    return {p: a for (p, _), a in zip(callee.params, ref[2])}


def _finish(d):
    """Parse the collected body lines into statements; `if COND` / `else` open indented blocks."""
    if d is None or not d._raw:
        return
    raw, base = d._raw, d._raw[0][0]
    d.src_lines["body"] = [line if ind == base else " " * (ind - base) + line for ind, line, _ in raw]
    stmts, i = _block(raw, 0, base)
    if i != len(raw):
        raise ParseError("unexpected indentation", raw[i][2])
    d.body = stmts


def _block(raw, i, indent):
    out = []
    while i < len(raw) and raw[i][0] == indent:
        _, line, no = raw[i]
        word, _, rest = line.partition(" ")
        if line == "else":
            raise ParseError("`else` without an `if`", no)
        loop = FOR_LOOP.match(line)
        if loop:                          # `for k from LO to HI`: leading `invariant` lines, then the body
            inner, i = _sub(raw, i + 1, indent, no)
            invs = [s[1] for s in inner if s[0] == "inv"]
            rest_ = [s for s in inner if s[0] != "inv"]
            if inner[:len(invs)] != [s for s in inner if s[0] == "inv"]:
                raise ParseError("loop `invariant` lines must come first in the loop", no)
            out.append(("for", loop.group(1), parse_expr(loop.group(2), no), parse_expr(loop.group(3), no),
                        invs, rest_))
            continue
        if word == "invariant":
            out.append(("inv", parse_clause(rest, no)))
            i += 1
            continue
        if word == "if":
            cond = parse_expr(rest, no)
            then, i = _sub(raw, i + 1, indent, no)
            other = []
            if i < len(raw) and raw[i][0] == indent and raw[i][1] == "else":
                other, i = _sub(raw, i + 1, indent, raw[i][2])
            out.append(("if", cond, then, other))
            continue
        if word == "need":
            out.append(parse_need(rest, no))
        elif re.match(r"^[a-z]\w*\s*=(?!=)", line):
            name, expr = line.split("=", 1)
            out.append(("let", name.strip(), parse_expr(expr, no)))
        else:
            out.append(("do", parse_expr(line, no)))
        i += 1
    if i < len(raw) and raw[i][0] > indent:
        raise ParseError("unexpected indentation", raw[i][2])
    return out, i


def _sub(raw, i, indent, no):
    if i >= len(raw) or raw[i][0] <= indent:
        raise ParseError("an `if` or `else` needs an indented block", no)
    return _block(raw, i, raw[i][0])


# ---------- printing back to code ----------

def code(e) -> str:
    k = e[0]
    if k == "int":
        return str(e[1])
    if k == "name":
        return e[1] + ("'" if e[2] else "")
    if k == "app":
        head = e[1][0] + ("'" if e[1][1] else "")
        return " ".join([head] + [atom_code(a) for a in e[2]])
    if k == "bin":
        return f"{_side(e[2], e[1], False, code)} {e[1]} {_side(e[3], e[1], True, code)}"
    if k == "all":
        return f"{code(e[4])} for {e[1]} from {atom_code(e[2])} to {atom_code(e[3])}"
    raise ValueError(e)


PREC = {"when": 0, "or": 1, "and": 2, "=": 3, "!=": 3, ">=": 3, "<=": 3, ">": 3, "<": 3, "+": 4, "-": 4, "*": 5, "/": 5}


def _side(child, op, right, show):
    """Print a binary operand, parenthesised when needed to reparse to the same tree."""
    text = show(child)
    c, p = (PREC[child[1]], PREC[op]) if child[0] == "bin" else (99, 0)
    # + - * / and or are left-associative; comparisons and `when` need parentheses on either side
    if c < p or (c == p and (right or op in CMP or op == "when")):
        return f"({text})"
    return text


NEG = {"=": "!=", "!=": "=", ">=": "<", "<=": ">", ">": "<=", "<": ">="}


def negate(e):
    """The negation as an expression (De Morgan over and/or), or None if there is none without `not`."""
    if e[0] != "bin":
        return None
    if e[1] in NEG:
        return ("bin", NEG[e[1]], e[2], e[3])
    if e[1] in ("and", "or"):
        l, r = negate(e[2]), negate(e[3])
        return ("bin", "or" if e[1] == "and" else "and", l, r) if l and r else None
    return None


def flat(stmts):
    """Every statement, including those inside `if` blocks (and the `if` itself)."""
    for s in stmts:
        yield s
        if s[0] == "if":
            yield from flat(s[2])
            yield from flat(s[3])
        elif s[0] == "for":
            yield from flat(s[5])


def atom_code(e):
    return code(e) if e[0] in ("int", "name") else f"({code(e)})"


def emit_code(ev) -> str:
    text = " ".join([ev[0]] + [atom_code(a) for a in ev[1]])
    return text + (f" when {code(ev[2])}" if len(ev) > 2 else "")


def stmt_code(s) -> str:
    if s[0] == "if":
        return f"if {code(s[1])}"
    if s[0] == "for":
        return f"for {s[1]} from {atom_code(s[2])} to {atom_code(s[3])}"
    if s[0] == "inv":
        return f"invariant {code(s[1])}"
    if s[0] == "let":
        return f"{s[1]} = {code(s[2])}"
    if s[0] == "need":
        fail = " ".join([s[2]] + [atom_code(a) for a in s[3]])
        return f"need {code(s[1])} else {fail}"
    return code(s[1])
