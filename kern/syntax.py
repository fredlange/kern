"""Lexer and parser for the Kern prototype.

A module is a sequence of definitions:

    name param:Type ... [-> Type]      header (column 0)
      need EXPR else Ctor ARGS          behavioural guard: fails with Ctor if EXPR is false
      keep EXPR                         EXPR has the same value before and after (on success)
      ensure EXPR                       postcondition; primed reads (x') see the after-state
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

TOKEN = re.compile(r"\s*(?:(\d+)|([A-Za-z_][A-Za-z0-9_.]*'?)|(!=|>=|<=|->|[=<>+\-()]))")
CMP = {"=", "!=", ">=", "<=", ">", "<"}


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
        left = self.sum()
        kind, val = self.peek()
        if kind == "op" and val in CMP:
            self.take()
            return ("bin", val, left, self.sum())
        if kind == "name" and val in ("and", "or"):
            self.take()
            return ("bin", val, left, self.expr())
        return left

    def sum(self):
        left = self.app()
        while self.peek() in (("op", "+"), ("op", "-")):
            op = self.take()[1]
            left = ("bin", op, left, self.app())
        return left

    def app(self):
        kind, val = self.peek()
        if kind == "name" and val not in ("and", "or", "else"):
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
        return kind == "int" or (kind == "name" and val not in ("and", "or", "else")) or (kind, val) == ("op", "(")

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


def parse_need(src: str, line_no):
    if " else " not in src:
        raise ParseError("need must say what it fails with: need COND else Ctor", line_no)
    cond, fail = src.rsplit(" else ", 1)
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


@dataclass
class Example:
    world: dict            # account -> (balance, owner)
    call: tuple            # (def name, [arg tokens as str|int])
    outcome: tuple         # ('ok',) or (Ctor, [ints])
    after: dict            # account -> balance
    src: str


@dataclass
class Def:
    name: str
    params: list           # [(name, type)]
    ret: str | None
    needs: list = field(default_factory=list)
    keeps: list = field(default_factory=list)
    ensures: list = field(default_factory=list)
    examples: list = field(default_factory=list)
    body: list = field(default_factory=list)
    src_lines: dict = field(default_factory=dict)   # section -> [source lines]

    @property
    def hash(self) -> str:
        canon = repr((self.params, self.ret, self.needs, self.keeps, self.ensures, self.body))
        return hashlib.sha256(canon.encode()).hexdigest()[:8]


WORLD_ITEM = re.compile(r"^([A-Z]\w*):(\d+)(?:@(\w+))?$")


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
    world = parse_world(parts[0], line_no)
    call_src, out_src = [s.strip() for s in parts[1].split("->")]
    call = call_src.split()
    args = [int(a) if a.isdigit() else a for a in call[1:]]
    out = out_src.split()
    outcome = ("ok",) if out == ["ok"] else (out[0], [int(x) for x in out[1:]])
    after = parse_world(parts[2], line_no, with_owner=False) if len(parts) == 3 else {}
    return Example(world, (call[0], args), outcome, after, src)


HEADER = re.compile(r"^([a-z]\w*)((?:\s+\w+:\w+)*)\s*(?:->\s*(\w+))?\s*$")


def parse_module(text: str) -> dict:
    defs, cur, in_body = {}, None, False
    for no, raw in enumerate(text.splitlines(), 1):
        if not raw.strip():
            continue
        line = raw.strip()
        if not raw[0].isspace():
            if line == "body":
                if cur is None:
                    raise ParseError("body without a definition", no)
                in_body = True
                continue
            m = HEADER.match(line)
            if not m:
                raise ParseError(f"bad definition header {line!r}", no)
            params = [tuple(p.split(":")) for p in m.group(2).split()]
            cur = Def(m.group(1), params, m.group(3))
            cur.src_lines = {"header": [line], "spec": [], "body": []}
            defs[cur.name] = cur
            in_body = False
            continue
        if cur is None:
            raise ParseError("indented line outside a definition", no)
        word, _, rest = line.partition(" ")
        if in_body:
            cur.src_lines["body"].append(line)
            if word == "need":
                cur.body.append(parse_need(rest, no))
            elif re.match(r"^[a-z]\w*\s*=(?!=)", line):
                name, expr = line.split("=", 1)
                cur.body.append(("let", name.strip(), parse_expr(expr, no)))
            else:
                cur.body.append(("do", parse_expr(line, no)))
            continue
        cur.src_lines["spec"].append(line)
        if word == "need":
            cur.needs.append(parse_need(rest, no))
        elif word == "keep":
            cur.keeps.append(parse_expr(rest, no))
        elif word == "ensure":
            cur.ensures.append(parse_expr(rest, no))
        elif word == "eg":
            cur.examples.append(parse_example(rest, no))
        else:
            raise ParseError(f"unknown clause {word!r} (need/keep/ensure/eg, then body)", no)
    return defs


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
        return f"{code(e[2])} {e[1]} {code(e[3])}"
    raise ValueError(e)


def atom_code(e):
    return code(e) if e[0] in ("int", "name") else f"({code(e)})"


def stmt_code(s) -> str:
    if s[0] == "let":
        return f"{s[1]} = {code(s[2])}"
    if s[0] == "need":
        fail = " ".join([s[2]] + [atom_code(a) for a in s[3]])
        return f"need {code(s[1])} else {fail}"
    return code(s[1])
