"""Lazy `why` store. Entries live outside the code, anchored to the hash of the definition
they explain; when the definition changes, its entries are reported as stale.

File format:
    transfer@cd25dc69
      entry text
      another entry

The anchor after `@` is the interface anchor (`anchor_hash`), not the build hash `check` prints; `kern anchor FILE`
writes it. Any token is accepted as an anchor (a placeholder is simply stale until anchored), but a line at column 0
that isn't `name@anchor` is an error, so no entry is ever dropped or attached to the wrong definition.
"""
from __future__ import annotations

import os
import re

HEAD = re.compile(r"^(\w+)@(\S+)$")


class WhyFileError(ValueError):
    pass


def load(path):
    store = {}
    if not path or not os.path.exists(path):
        return store
    cur = None
    for no, line in enumerate(open(path), 1):
        if not line.strip():
            continue
        if not line[0].isspace():
            m = HEAD.match(line.strip())
            if not m:
                raise WhyFileError(f"{path}:{no}: want `name@anchor` at column 0, got {line.strip()!r}")
            cur = store.setdefault(m.group(1), {"hash": m.group(2), "entries": []})
        elif cur is None:
            raise WhyFileError(f"{path}:{no}: an entry before any `name@anchor` line")
        else:
            cur["entries"].append(line.strip())
    return store


OBLIGATION = re.compile(r"\b(must|never|always|only|required|do not|don't|mustn't|shall)\b", re.I)


def anchor_hash(defs, d):
    """What a why entry is anchored to: the definition's own text *and* its inferred interface
    (uses, fails, writes, lifted refusal conditions). A callee change that alters the caller's
    interface therefore marks the caller's why stale, even though the caller's text is unchanged."""
    import hashlib
    from .infer import fmt_fails, fmt_uses, fails, lifted, uses, writes
    from .syntax import code
    iface = (d.hash, fmt_uses(uses(defs, d)), fmt_fails(fails(defs, d)), sorted(writes(defs, d)),
             [(code(c), k) for c, k, *_ in lifted(defs, d)])
    return hashlib.sha256(repr(iface).encode()).hexdigest()[:8]


def status(store, d, defs):
    s = store.get(d.name)
    if not s:
        return {"count": 0, "stale": False, "entries": [], "obligations": []}
    return {"count": len(s["entries"]), "stale": s["hash"] != anchor_hash(defs, d), "entries": s["entries"],
            "anchored_to": s["hash"], "current": anchor_hash(defs, d),
            # rule 3: nothing that must be obeyed may live only in prose (heuristic, a warning)
            "obligations": [e for e in s["entries"] if OBLIGATION.search(e)]}


def orphans(store, defs):
    """Why entries whose definition no longer exists (renamed or deleted)."""
    return {n: s["entries"] for n, s in store.items() if n not in defs}


def anchor(path, defs):
    """Re-anchor every entry to the current interface. Orphans are kept (never silently
    deleted) and returned, so the caller can warn about them."""
    store = load(path)
    with open(path, "w") as f:
        for name, s in store.items():
            h = anchor_hash(defs, defs[name]) if name in defs else s["hash"]
            f.write(f"{name}@{h}\n" + "".join(f"  {e}\n" for e in s["entries"]))
    return orphans(store, defs)
