"""An MCP server for Kern: an agent works through views, never through whole files.

    python -m kern.mcp            (stdio, JSON-RPC 2.0, one message per line; no dependencies)

Tools (see docs/GUIDE.md):
  kern_list     definitions with their interface line and status
  kern_view     iface | spec | editor view of one definition (identical to `kern view`)
  kern_why      rationale entries for a definition
  kern_check    check a module (quick by default, cached); terminal text + per-definition status
  kern_run      one call in the interpreter
  kern_add      add a definition
  kern_replace  replace one definition
  kern_remove   remove a definition (refused while it is referenced)

Edits are structural. The whole new module is parsed and checked for well-formedness first; if
it is ill-formed, the file is unchanged. Otherwise the file is written, and the edited
definition and every transitive caller are re-checked (callers were verified against the old
spec). The result is ok only if all of them are clean; otherwise it says what broke, as `check`
does for a file.
"""
from __future__ import annotations

import json
import os
import re
import sys

from . import render, views
from . import why as whymod
from .infer import callees
from .report import analyse
from .run import Interp, World
from .syntax import ParseError, parse_module, parse_world, split_maps, split_now
from .wf import check_module
from . import package

PROTOCOL = "2024-11-05"
FILE = {"type": "string", "description": "path to the .k module"}
NAME = {"type": "string", "description": "definition name"}
TOOLS = [
    {"name": "kern_list", "description": "List the definitions of a module: interface line and status.",
     "inputSchema": {"type": "object", "properties": {"file": FILE}, "required": ["file"]}},
    {"name": "kern_view", "description": "A view of one definition: iface (what callers need), spec (+ guarantees), "
                                         "editor (its full source + its callees' spec views).",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "name": NAME,
                                                      "as": {"type": "string", "enum": ["iface", "spec", "editor"]}},
                     "required": ["file", "name"]}},
    {"name": "kern_why", "description": "Rationale (why) entries for a definition, with staleness.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "name": NAME}, "required": ["file", "name"]}},
    {"name": "kern_check", "description": "Check a module. quick (default true) skips mutation testing. Without "
                                          "name: a summary (status per definition, plus what is broken); with name: full detail.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "quick": {"type": "boolean"}, "name": NAME},
                     "required": ["file"]}},
    {"name": "kern_run", "description": "Run one call in the interpreter, e.g. call 'transfer A B 100', world 'A:500@u1 B:0@u2'.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "call": {"type": "string"}, "world": {"type": "string"}},
                     "required": ["file", "call", "world"]}},
    {"name": "kern_callers", "description": "Definitions that call (reach) this one, transitively: whose guarantees a change can break.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "name": NAME}, "required": ["file", "name"]}},
    {"name": "kern_edit", "description": "Apply several edits together, then check once: ops = [{op: add, source} | "
                                         "{op: replace, name, source, expect_hash?} | {op: remove, name, expect_hash?}]. "
                                         "If the result is ill-formed, nothing is written.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "ops": {"type": "array", "items": {"type": "object"}}},
                     "required": ["file", "ops"]}},
    {"name": "kern_add", "description": "Add one definition (its full source: header, spec, body).",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "source": {"type": "string"}}, "required": ["file", "source"]}},
    {"name": "kern_replace", "description": "Replace one definition with new source (same name). expect_hash: refuse "
                                            "if its build hash is no longer this.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "name": NAME, "source": {"type": "string"},
                                                      "expect_hash": {"type": "string"}},
                     "required": ["file", "name", "source"]}},
    {"name": "kern_remove", "description": "Remove one definition; refused while anything references it.",
     "inputSchema": {"type": "object", "properties": {"file": FILE, "name": NAME, "expect_hash": {"type": "string"}},
                     "required": ["file", "name"]}},
]


class ToolError(Exception):
    pass


# --- the module as text: definitions are column-0 blocks ------------------------------------

def _span(text, name):
    """(start, end) line indexes of definition `name`'s block (header to the line before the next
    column-0 line that isn't `body`), trailing blank lines excluded."""
    lines = text.split("\n")
    start = next((i for i, l in enumerate(lines) if re.match(rf"^{re.escape(name)}(\s|$)", l)), None)
    if start is None:
        raise ToolError(f"no definition named {name!r}")
    end = start + 1
    while end < len(lines) and not (lines[end][:1].strip() and lines[end].strip() != "body"):
        end += 1
    while end > start + 1 and not lines[end - 1].strip():
        end -= 1
    return lines, start, end


def _one_def(source):
    """The name of the single definition in `source`. Syntax errors are reported with line numbers
    relative to the submitted definition (the agent never sees file lines); references to other
    definitions (`need as F`) are resolved later, against the whole module."""
    heads = [l for l in source.strip("\n").split("\n") if l[:1].strip() and l.strip() != "body"]
    if len(heads) != 1:
        raise ToolError("the source must be exactly one definition (one header line at column 0)")
    m = re.match(r"^([a-z]\w*)", heads[0])
    if not m or heads[0].startswith(("state ", "invariant ", "channel ")):
        raise ToolError(f"not a definition header: {heads[0]!r}")
    try:
        parse_module(source)
    except ParseError as e:
        if "`as " not in str(e):
            raise ToolError(f"does not parse (lines count from the submitted definition): {e}")
    return m.group(1)


def _inside_root(path):
    """KERN_MCP_ROOT=dir: the server only touches modules under dir (C2's isolation)."""
    root = os.environ.get("KERN_MCP_ROOT")
    if root and not os.path.realpath(path).startswith(os.path.realpath(root) + os.sep):
        raise ToolError(f"{path} is outside this server's root")


def _load(path):
    _inside_root(path)
    text = open(path).read()
    try:
        defs = package.load(path)                  # a package (kern.pkg) or imports: every file it needs
        for f in package.files_of(defs).values():   # ... all of them inside this server's root
            _inside_root(f)
        return text, defs
    except ParseError as e:
        raise ToolError(f"{path}: {e}")


def _whys(path, defs):
    store = whymod.load(package.store_path(path, ".why"))
    return {n: whymod.status(store, d, defs) for n, d in defs.items()}


def _cache(path):
    return os.path.join(os.path.dirname(os.path.abspath(path)), ".kern-cache")


def _status(r):
    return render._status(r)[1]


def _callers(defs, name):
    """Every definition that reaches `name`, transitively."""
    out, frontier = set(), {name}
    while frontier:
        new = {n for n, d in defs.items() if n not in out and frontier & set(callees(defs, d))}
        out |= new
        frontier = new
    return out


def _apply(text, defs, op, file=None):
    """One structural edit on the module text. Returns (new text, touched definition or None)."""
    kind = op.get("op")
    name = op.get("name")
    if kind in ("replace", "remove"):
        if name not in defs:
            raise ToolError(f"no definition named {name!r}")
        home = getattr(defs[name], "file", None)
        if home and file and os.path.abspath(home) != os.path.abspath(file):
            raise ToolError(f"{name} is defined in {home}; pass that file")
        if op.get("expect_hash") and defs[name].hash != op["expect_hash"]:
            raise ToolError(f"{name} changed underneath: its build hash is {defs[name].hash}, not {op['expect_hash']}")
    if kind == "add":
        new = _one_def(op.get("source", ""))
        if new in defs:
            raise ToolError(f"{new} already exists; use replace")
        return text.rstrip("\n") + "\n\n" + op["source"].strip("\n") + "\n", new
    if kind == "replace":
        got = _one_def(op.get("source", ""))
        if got != name:
            raise ToolError(f"the source defines {got!r}, not {name!r} (renaming isn't an edit here)")
        lines, start, end = _span(text, name)
        return "\n".join(lines[:start] + op["source"].strip("\n").split("\n") + lines[end:]), name
    if kind == "remove":
        users = sorted(n for n in _callers(defs, name) if n != name)
        refs = sorted(n for n, d in defs.items() if name in d.ref_origin.values() and n != name)
        if users or refs:
            raise ToolError(f"refused: {', '.join(sorted(set(users + refs)))} reference {name}")
        lines, start, end = _span(text, name)
        while end < len(lines) and not lines[end].strip():
            end += 1
        return "\n".join(lines[:start] + lines[end:]).rstrip("\n") + "\n", None
    raise ToolError(f"unknown op {kind!r} (add, replace, remove)")


def kern_edit(file, ops):
    """Apply ops in order, validate the whole result once; ill-formed -> nothing written. Then write,
    re-check the touched definitions and every transitive caller, and report what broke."""
    text, defs = _load(file)
    old_whys = _whys(file, defs)
    touched = []
    for op in ops:
        text, t = _apply(text, defs, op, file)
        try:                                    # later ops see earlier ones; full check below
            defs = package.load(file, overrides={file: text}) if package.is_package_file(file, text) \
                else parse_module(text)
        except ParseError as e:
            raise ToolError(f"rejected, file unchanged: {e}")
        if t:
            touched.append(t)
    try:
        defs = check_module(defs)
    except ParseError as e:
        raise ToolError(f"rejected, file unchanged: {e}")
    with open(file, "w") as f:
        f.write(text)
    affected = set(touched) & set(defs)
    for t in touched:
        affected |= _callers(defs, t)
    reps = analyse(defs, mutation=False, cache=_cache(file))
    whys = _whys(file, defs)
    report = {n: _status(reps[n]) for n in sorted(affected)}
    broken = {n: s for n, s in report.items() if s != "All guarantees proven"}
    store = whymod.load(package.store_path(file, ".why"))
    return {"ok": not broken, "affected": report, "broken": sorted(broken),
            "rechecked": sorted(n for n, r in reps.items() if not r["cached"]),
            "details": render.terminal({n: reps[n] for n in broken}, whys) if broken else "",
            "why_now_stale": sorted(n for n, w in whys.items() if w["stale"] and not old_whys.get(n, {}).get("stale")),
            "why_orphaned": sorted(whymod.orphans(store, defs))}


# --- tools -----------------------------------------------------------------------------------

def kern_list(file):
    _, defs = _load(file)
    reps = analyse(defs, mutation=False, cache=_cache(file))
    whys = _whys(file, defs)
    return {"definitions": [{"name": n, "iface": views.iface(r, whys[n]).split("\n")[0], "status": _status(r)}
                            for n, r in reps.items()]}


def kern_view(file, name, **kw):
    as_ = kw.get("as", "iface")
    _, defs = _load(file)
    if name not in defs:
        raise ToolError(f"no definition named {name!r}")
    reps = analyse(defs, mutation=False, cache=_cache(file))
    whys = _whys(file, defs)
    text = {"iface": lambda: views.iface(reps[name], whys[name]), "spec": lambda: views.spec(reps[name], whys[name]),
            "editor": lambda: views.editor(reps, name, whys)}[as_]()
    return {"text": text, "tokens": views.tokens(text)}


def kern_why(file, name):
    _, defs = _load(file)
    if name not in defs:
        raise ToolError(f"no definition named {name!r}")
    w = _whys(file, defs)[name]
    return {"entries": w["entries"], "stale": w["stale"]}


def kern_check(file, quick=True, name=None):
    _, defs = _load(file)
    reps = analyse(defs, mutation=not quick, cache=_cache(file))
    whys = _whys(file, defs)
    if name is not None:
        if name not in reps:
            raise ToolError(f"no definition named {name!r}")
        return {"text": render.terminal({name: reps[name]}, whys), "status": _status(reps[name])}
    bad = {n: r for n, r in reps.items() if not r["clean"]}
    return {"clean": not bad, "status": {n: _status(r) for n, r in reps.items()},
            "problems": render.terminal(bad, whys) if bad else ""}


def kern_run(file, call, world):
    _, defs = _load(file)
    rest, now = split_now(world)
    rest, maps = split_maps(rest)
    w = World.from_example(parse_world(rest, None), now, maps)
    name, *args = call.split()
    if name not in defs:
        raise ToolError(f"no definition named {name!r}")
    args = [int(x) if x.lstrip("-").isdigit() else x for x in args]
    outcome, after = Interp(defs).run(name, args, w)
    return {"outcome": "ok" if outcome == ("ok",) else " ".join([outcome[0], *map(str, outcome[1])]),
            "balances": after.bal, "events": [list(e) for e in after.audit],
            "state": {s: {str(k): v for k, v in m.items()} for s, m in after.maps.items()}}


def kern_add(file, source):
    return kern_edit(file, [{"op": "add", "source": source}])


def kern_replace(file, name, source, expect_hash=None):
    return kern_edit(file, [{"op": "replace", "name": name, "source": source, "expect_hash": expect_hash}])


def kern_remove(file, name, expect_hash=None):
    return kern_edit(file, [{"op": "remove", "name": name, "expect_hash": expect_hash}])


def kern_callers(file, name):
    _, defs = _load(file)
    if name not in defs:
        raise ToolError(f"no definition named {name!r}")
    direct = sorted(n for n, d in defs.items() if name in callees(defs, d))
    return {"direct": direct, "transitive": sorted(_callers(defs, name))}


HANDLERS = {"kern_list": kern_list, "kern_view": kern_view, "kern_why": kern_why, "kern_check": kern_check,
            "kern_run": kern_run, "kern_callers": kern_callers, "kern_edit": kern_edit, "kern_add": kern_add,
            "kern_replace": kern_replace, "kern_remove": kern_remove}


# --- JSON-RPC over stdio ---------------------------------------------------------------------

def _log(tool, args, text):
    """KERN_MCP_LOG=path: one JSON line per tool call with the tokens it returned (for C2)."""
    path = os.environ.get("KERN_MCP_LOG")
    if path:
        with open(path, "a") as f:
            f.write(json.dumps({"tool": tool, "args": {k: (v if len(str(v)) < 200 else str(v)[:200] + "…")
                                                       for k, v in args.items()},
                                "response_tokens": views.tokens(text), "response_chars": len(text)}) + "\n")


def handle(msg):
    """One JSON-RPC message -> the response dict, or None for a notification."""
    mid, method, params = msg.get("id"), msg.get("method"), msg.get("params") or {}
    if mid is None:
        return None                                            # notifications (e.g. initialized)
    if method == "initialize":
        result = {"protocolVersion": params.get("protocolVersion", PROTOCOL), "capabilities": {"tools": {}},
                  "serverInfo": {"name": "kern", "version": "0.1"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        fn = HANDLERS.get(params.get("name"))
        if fn is None:
            return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": f"unknown tool {params.get('name')!r}"}}
        try:
            out = fn(**(params.get("arguments") or {}))
            result = {"content": [{"type": "text", "text": json.dumps(out, indent=1)}], "isError": False}
            _log(params.get("name"), params.get("arguments") or {}, result["content"][0]["text"])
        except (ToolError, TypeError, OSError, KeyError) as e:
            result = {"content": [{"type": "text", "text": f"{type(e).__name__}: {e}"}], "isError": True}
    elif method == "ping":
        result = {}
    else:
        return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": f"no method {method!r}"}}
    return {"jsonrpc": "2.0", "id": mid, "result": result}


def main(inp=sys.stdin, out=sys.stdout):
    for line in inp:
        if not line.strip():
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            out.write(json.dumps({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "parse error"}}) + "\n")
            out.flush()
            continue
        resp = handle(msg)
        if resp is not None:
            out.write(json.dumps(resp) + "\n")
            out.flush()


if __name__ == "__main__":
    main()
