"""Lazy `why` store. Entries live outside the code, anchored to the hash of the definition
they explain; when the definition changes, its entries are reported as stale.

File format:
    transfer@cd25dc69
      entry text
      another entry
"""
from __future__ import annotations

import os
import re

HEAD = re.compile(r"^(\w+)@([0-9a-f]{8})$")


def load(path):
    store = {}
    if not path or not os.path.exists(path):
        return store
    cur = None
    for line in open(path):
        if not line.strip():
            continue
        m = HEAD.match(line.strip())
        if m and not line[0].isspace():
            cur = store.setdefault(m.group(1), {"hash": m.group(2), "entries": []})
        elif cur is not None:
            cur["entries"].append(line.strip())
    return store


def status(store, d):
    s = store.get(d.name)
    if not s:
        return {"count": 0, "stale": False, "entries": []}
    return {"count": len(s["entries"]), "stale": s["hash"] != d.hash, "entries": s["entries"],
            "anchored_to": s["hash"]}


def anchor(path, defs):
    store = load(path)
    with open(path, "w") as f:
        for name, s in store.items():
            h = defs[name].hash if name in defs else s["hash"]
            f.write(f"{name}@{h}\n" + "".join(f"  {e}\n" for e in s["entries"]))
