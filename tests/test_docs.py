"""The docs stay current: docs/REFERENCE.md and docs/GUIDE.md are checked against the code.

- every ```kern block parses and passes the checker; ```kern clean blocks also prove clean
- every handler, spec clause and declaration form is documented in REFERENCE.md
- every CLI command and option of `kern`, `kern.host` and `kern.emit_rust` is documented in GUIDE.md, and
  the fuzzer's in validation/README.md
- every `kern.module.name` in GUIDE.md's API table exists
- REFERENCE.md stays within its token budget (docs/research/CHALLENGE.md, C2: 2,500)
If one of these fails, the docs are behind the code: update them in the same commit.
"""
from __future__ import annotations

import importlib
import re

import pytest

from kern import views
from kern.report import analyse
from kern.run import HANDLERS
from kern.syntax import parse_module
from kern.wf import check_module

REF = open("docs/REFERENCE.md").read()
GUIDE = open("docs/GUIDE.md").read()
BLOCKS = [(f, info, body) for f, text in (("REFERENCE.md", REF), ("GUIDE.md", GUIDE))
          for info, body in re.findall(r"```(kern[^\n]*)\n(.*?)```", text, re.S)]


def test_there_are_kern_examples():
    assert len(BLOCKS) >= 2


@pytest.mark.parametrize("where,info,body", BLOCKS, ids=[f"{f}:{i}" for i, (f, _, _) in enumerate(BLOCKS)])
def test_every_kern_block_checks(where, info, body):
    defs = check_module(parse_module(body))
    if "clean" in info.split():
        reps = analyse(defs, mutation=False)
        assert all(r["clean"] for r in reps.values()), {n: r["guarantees"] for n, r in reps.items() if not r["clean"]}


@pytest.mark.parametrize("name", sorted(HANDLERS))
def test_every_handler_is_documented(name):
    assert f"`{name}" in REF


@pytest.mark.parametrize("form", ["need", "ok otherwise", "keep", "ensure", "emit", "when", "need as", "emit as",
                                  "eg", "state", "invariant", "channel", "-> Int", "result", "distinct", "if", "else"])
def test_every_language_form_is_documented(form):
    assert f"`{form}" in REF


def _cli(path):
    src = open(path).read()
    m = re.search(r'add_argument\("cmd", choices=\[([^\]]*)\]', src)
    cmds = re.findall(r'"(\w+)"', m.group(1)) if m else []
    opts = [o for o, rest in re.findall(r'add_argument\("(--[\w-]+)"(.*)', src) if "SUPPRESS" not in rest]
    return cmds, opts


@pytest.mark.parametrize("path,doc,section", [
    ("kern/__main__.py", "docs/GUIDE.md", "## `python -m kern` commands"),
    ("kern/host.py", "docs/GUIDE.md", "## `python -m kern.host` commands"),
    ("kern/emit_rust.py", "docs/GUIDE.md", "## `python -m kern.emit_rust`"),
    ("validation/fuzz.py", "validation/README.md", "## `python -m validation.fuzz`")])
def test_every_cli_command_and_option_is_documented(path, doc, section):
    part = open(doc).read().split(section, 1)[1].split("\n## ", 1)[0]
    cmds, opts = _cli(path)
    for c in cmds:
        assert f"| `{c} " in part or f"| `{c}`" in part, f"command {c} of {path} is not in {doc}"
    for o in opts:
        assert f"`{o}" in part, f"option {o} of {path} is not in {doc}"


API = sorted(set(re.findall(r"`(kern\.[a-z_]+\.[A-Za-z_]+)", GUIDE.split("## Python API", 1)[1].split("\n## ", 1)[0])))


@pytest.mark.parametrize("dotted", API)
def test_every_api_name_exists(dotted):
    mod, _, name = dotted.rpartition(".")
    assert hasattr(importlib.import_module(mod), name), f"{dotted} is in GUIDE.md but not in the code"


def test_reference_stays_within_its_token_budget():
    n = views.tokens(REF)
    if n is None:
        pytest.skip("no tokenizer")
    assert n <= 2500, f"REFERENCE.md is {n} tokens; the budget is 2,500 (docs/research/CHALLENGE.md, C2)"


def test_every_mcp_tool_is_documented():
    from kern import mcp
    part = GUIDE.split("## The MCP server", 1)[1].split("\n## ", 1)[0]
    for t in mcp.TOOLS:
        assert f"`{t['name']}`" in part, f"MCP tool {t['name']} is not in GUIDE.md"
