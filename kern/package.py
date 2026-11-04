"""Packages and imports (docs/research/ROADMAP_RUN.md, P; a proposal). Loading only: the files of a package merge
into one Module, each definition knowing its file, and `import a.b` controls what a file may use. The
prover, interpreter and host see one Module, as before.

- A package is a directory with a `kern.pkg` file. Loading any file in it loads every `.k` file under it.
- A module is one file, named by its dotted path from the package root (`billing/invoice.k` is
  `billing.invoice`). Outside a package, a file loads itself plus what it imports, from its directory.
- A file may use only the definitions, state, handlers and channels it declares or imports directly.
  Names are unique across the package.
"""
import os
import re

from .syntax import Module, ParseError, expand_refs, parse_module
from .wf import check_module

MARKER = "kern.pkg"
NAME = re.compile(r"^[a-z]\w*(?:\.[a-z]\w*)*$")


class LoadError(ParseError):
    """A package-level error, with the file (and line) it is about."""

    def __init__(self, path, msg, line_no=None):
        Exception.__init__(self, f"{path}:{line_no}: {msg}" if line_no else f"{path}: {msg}")


def root_of(path):
    """The package root: the nearest directory, from the file's up, holding a kern.pkg. None outside one."""
    d = os.path.abspath(path) if os.path.isdir(path) else os.path.dirname(os.path.abspath(path))
    while True:
        if os.path.exists(os.path.join(d, MARKER)):
            return d
        up = os.path.dirname(d)
        if up == d:
            return None
        d = up


def module_name(base, path):
    return os.path.relpath(os.path.abspath(path), base)[:-len(".k")].replace(os.sep, ".")


def module_path(base, name):
    return os.path.join(base, *name.split(".")) + ".k"


def split_imports(text, path):
    """[(module, line)] and the text with the import lines blanked (line numbers stay right)."""
    imports, lines = [], []
    for no, raw in enumerate(text.split("\n"), 1):
        if raw.startswith("import ") or raw.strip() == "import":
            name = raw[len("import "):].strip()
            if not NAME.match(name):
                raise LoadError(path, f"want `import a.b` (a module name, relative to the package root), got {raw!r}", no)
            imports.append((name, no))
            lines.append("")
        else:
            lines.append(raw)
    return imports, "\n".join(lines)


def is_package_file(path, text=None):
    """Whether loading this file needs the package loader (inside a package, or it imports)."""
    if root_of(path):
        return True
    text = open(path).read() if text is None else text
    return any(l.startswith("import ") for l in text.split("\n"))


def load(path, overrides=None):
    """The checked Module for the package (or import closure) `path` belongs to. overrides = {path: text}
    replaces a file's text in memory (an edit being validated)."""
    overrides = {os.path.abspath(p): t for p, t in (overrides or {}).items()}
    path = os.path.abspath(path)
    if os.path.isdir(path):                                  # the package directory itself
        path = os.path.join(path, MARKER)

    def read(p):
        return overrides[p] if p in overrides else open(p).read()
    root = root_of(path)
    if root is None and not is_package_file(path, read(path)):
        return check_module(parse_module(read(path)))        # a plain single-file module, exactly as before
    base = root or os.path.dirname(path)
    if root:
        todo = sorted(os.path.join(d, f) for d, ds, fs in os.walk(root) for f in fs
                      if f.endswith(".k") and not any(part.startswith(".") for part in os.path.relpath(d, root).split(os.sep)
                                                      if part != "."))
    else:
        todo = [path]
    files, imports, seen = {}, {}, set()
    while todo:
        p = todo.pop(0)
        if p in seen:
            continue
        seen.add(p)
        mod = module_name(base, p)
        imps, text = split_imports(read(p), p)
        try:
            defs = parse_module(text, expand=False)
        except ParseError as e:
            raise LoadError(p, str(e)) from None
        files[mod], imports[mod] = (p, defs, read(p)), imps
        for name, no in imps:
            target = module_path(base, name)
            if not os.path.exists(target) and target not in overrides:
                raise LoadError(p, f"no module {name} (looked for {os.path.relpath(target, base)})", no)
            if not root:
                todo.append(target)
    merged, origin = Module(), {"def": {}, "state": {}, "handler": {}, "channel": {}}
    for mod in sorted(files):
        p, defs, text = files[mod]
        for kind, names in (("state", defs.states), ("handler", defs.handlers), ("channel", defs.channels)):
            for n in names:
                if n in origin[kind]:
                    raise LoadError(p, f"{kind} {n} is already declared in {origin[kind][n]}")
                origin[kind][n] = mod
        merged.states.update(defs.states)
        merged.handlers.update(defs.handlers)
        merged.channels.extend(defs.channels)
        merged.invariants.extend(defs.invariants)
        for n, d in list(defs.items()) + list(defs.handlers.items()):
            d.module, d.file, d.line = mod, p, _header_line(text, d)
        for n, d in defs.items():
            if n in merged or n in origin["handler"]:
                other = origin["def"].get(n) or origin["handler"].get(n)
                raise LoadError(p, f"{n} is already defined in {other}", d.line)
            merged[n] = d
            origin["def"][n] = mod
    try:
        expand_refs(merged)
    except ParseError as e:
        raise LoadError(path, str(e)) from None
    for mod, (p, defs, _) in files.items():
        visible = {mod} | {name for name, _ in imports[mod]}
        for d in defs.values():
            for kind, n in _uses(merged, d):
                owner = origin[kind].get(n)
                if owner and owner not in visible:
                    raise LoadError(p, f"{d.name} uses {kind} {n}, which {owner} declares: add `import {owner}`", d.line)
    merged.package = {"root": root, "base": base, "files": {m: f[0] for m, f in files.items()},
                      "imports": {m: [n for n, _ in i] for m, i in imports.items()}, "origin": origin}
    return check_module(merged)


def _header_line(text, d):
    head = d.src_lines.get("header", [""])[0]
    for no, raw in enumerate(text.split("\n"), 1):
        if raw.rstrip() == head:
            return no
    return None


def _uses(defs, d):
    """(kind, name) for every definition, state, handler and channel d's spec or body names."""
    from .infer import _stmt_exprs, _walk
    from .syntax import flat
    exprs = [n[1] for n in d.needs if len(n) > 1] + [a for n in d.needs if len(n) > 3 for a in n[3]] + \
        list(d.keeps) + list(d.ensures)
    for ev in d.emits:
        exprs += [a for a in ev[1]] + ([ev[2]] if len(ev) > 2 and ev[2] is not None else [])
    for s in flat(d.body):
        exprs += list(_stmt_exprs(s))
    out = set(("def", c) for c in d.ref_origin.values())
    for e in exprs:
        if not isinstance(e, tuple):
            continue
        for n in _walk(e):
            name = n[1][0] if n[0] == "app" else n[1] if n[0] == "name" else None
            if not isinstance(name, str):
                continue
            x, _, op = name.rpartition(".")
            if name in defs and name != d.name:
                out.add(("def", name))
            elif name in defs.handlers:
                out.add(("handler", name))
            elif x in defs.states and op in ("get", "set"):
                out.add(("state", x))
            elif x in defs.channels and op == "emit":
                out.add(("channel", x))
    return sorted(out)


def store_path(path, ext):
    """Where the package's .why / .strength lives: <root>/kern.<ext> in a package, else next to the file."""
    root = root_of(path)
    return os.path.join(root, "kern" + ext) if root else os.path.splitext(path)[0] + ext


def files_of(defs):
    """{module: path} of a loaded package, or {} for a single file."""
    return getattr(defs, "package", {}).get("files", {})
