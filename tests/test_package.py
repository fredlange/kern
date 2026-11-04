"""Packages and imports (docs/research/ROADMAP_RUN.md, P): the loader, its errors, the tools across files, the host."""
import os
import shutil
import subprocess
import sys

import pytest

from kern import mcp, package
from kern.report import analyse
from kern.syntax import ParseError

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APPS = ("shop", "library", "lottery")


def _copy(tmp_path, app):
    dst = tmp_path / app
    shutil.copytree(os.path.join(ROOT, "examples", app), dst, ignore=shutil.ignore_patterns(".kern-cache"))
    return dst


def _write(root, files, pkg=True):
    root.mkdir(parents=True, exist_ok=True)
    if pkg:
        (root / "kern.pkg").write_text("")
    for rel, src in files.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(src)
    return root


LEDGER = open(os.path.join(ROOT, "examples/shop/money/ledger.k")).read()
PAY = """import money.ledger

pay a:Account b:Account
  need as transfer a b 5
  ok otherwise
  keep ledger.total
  emit as transfer a b 5
  eg A:10@u1 B:0@u2 | pay A B -> ok | A:5 B:5
body
  transfer a b 5
"""


# ---- condition 1: the loader -------------------------------------------------------------------

@pytest.mark.parametrize("app", APPS)
def test_every_example_package_loads_and_is_clean(app):
    defs = package.load(os.path.join(ROOT, "examples", app))
    files = package.files_of(defs)
    assert len(files) >= 3 and all(os.path.exists(f) for f in files.values())
    assert all(getattr(d, "file", None) in files.values() for d in defs.values())
    reps = analyse(defs, mutation=False)
    assert all(r["clean"] for r in reps.values()), [n for n, r in reps.items() if not r["clean"]]


def test_any_file_or_the_directory_loads_the_same_package():
    names = {tuple(sorted(package.load(os.path.join(ROOT, "examples/shop", f)))) for f in
             ("orders.k", "money/ledger.k", "inventory/catalog.k", ".", "kern.pkg")}
    assert len(names) == 1


def test_a_name_used_but_not_imported_is_an_error(tmp_path):
    root = _write(tmp_path / "p", {"money/ledger.k": LEDGER, "pay.k": PAY.replace("import money.ledger\n", "")})
    with pytest.raises(ParseError, match=r"pay.k:\d+: pay uses def transfer, which money.ledger declares: add `import money.ledger`"):
        package.load(root / "pay.k")


def test_state_from_a_file_not_imported_is_an_error(tmp_path):
    cat = "state stock Int\n\nrestock item:Int\n  ok otherwise\nbody\n  stock.set item 1\n"
    sell = "sell item:Int\n  ok otherwise\nbody\n  stock.set item 0\n"
    root = _write(tmp_path / "p", {"catalog.k": cat, "sell.k": sell})
    with pytest.raises(ParseError, match="uses state stock, which catalog declares"):
        package.load(root / "sell.k")


def test_an_unknown_module_is_an_error_with_file_and_line(tmp_path):
    root = _write(tmp_path / "p", {"pay.k": PAY.replace("import money.ledger", "import money.ledgr")})
    with pytest.raises(ParseError, match=r"pay.k:1: no module money.ledgr \(looked for money/ledgr.k\)"):
        package.load(root / "pay.k")


def test_a_duplicate_name_across_files_is_an_error(tmp_path):
    root = _write(tmp_path / "p", {"a.k": LEDGER, "b.k": LEDGER})
    with pytest.raises(ParseError, match="transfer is already defined in a"):
        package.load(root / "a.k")


@pytest.mark.parametrize("bad", ["import ../ledger", "import /etc/passwd", "import money..ledger", "import Money"])
def test_an_import_can_only_name_a_module(tmp_path, bad):
    """Isolation: module names are dotted identifiers, so nothing outside the package can be named."""
    root = _write(tmp_path / "p", {"money/ledger.k": LEDGER, "pay.k": PAY.replace("import money.ledger", bad)})
    with pytest.raises(ParseError, match="want `import a.b`"):
        package.load(root / "pay.k")


def test_outside_a_package_a_file_loads_what_it_imports(tmp_path):
    root = _write(tmp_path / "loose", {"money/ledger.k": LEDGER, "pay.k": PAY}, pkg=False)
    defs = package.load(root / "pay.k")
    assert sorted(defs) == ["pay", "transfer"] and all(r["clean"] for r in analyse(defs, mutation=False).values())


def test_a_single_file_without_imports_is_loaded_exactly_as_before():
    from kern.syntax import parse_module
    from kern.wf import check_module
    path = os.path.join(ROOT, "examples/escrow/escrow_s1.k")
    a, b = package.load(path), check_module(parse_module(open(path).read()))
    assert [d.hash for d in a.values()] == [d.hash for d in b.values()] and not hasattr(a, "package")


def test_import_in_a_single_file_parse_says_to_use_the_loader():
    from kern.syntax import parse_module
    with pytest.raises(ParseError, match="needs the package loader"):
        parse_module(PAY)


# ---- condition 2: the tools --------------------------------------------------------------------

def _kern(*args, cwd=ROOT):
    return subprocess.run([sys.executable, "-m", "kern", *args], cwd=cwd, capture_output=True, text=True,
                          env=dict(os.environ, PYTHONPATH=ROOT))


def test_cli_check_on_any_file_checks_the_package_and_names_the_files():
    p = _kern("check", "examples/library/fines.k", "--quick")
    assert p.returncode == 0, p.stdout + p.stderr
    for mod in ("== catalog.books", "== loans", "== fines", "== money.ledger"):
        assert mod in p.stdout
    v = _kern("view", "examples/library/fines.k", "borrow", "--quick")        # defined in loans.k
    assert v.returncode == 0 and v.stdout.splitlines()[0].startswith("borrow loan:Int book:Int")


def test_mcp_lists_views_and_checks_a_package(tmp_path):
    root = _copy(tmp_path, "shop")
    names = {d["name"] for d in mcp.kern_list(str(root / "orders.k"))["definitions"]}
    assert names == {"buy", "refund", "transfer", "set_price", "restock", "restock_range", "discount_range"}
    assert "transfer from:Account" in mcp.kern_view(str(root / "orders.k"), "transfer", **{"as": "iface"})["text"]


def test_mcp_replace_writes_the_defining_file_and_rechecks_callers_in_other_files(tmp_path):
    root = _copy(tmp_path, "shop")
    ledger = root / "money" / "ledger.k"
    before_orders = (root / "orders.k").read_text()
    weak = LEDGER.split("body\n")[0].replace("  need from != to else SameAccount\n", "") + \
        "body\n  ledger.debit from amount\n  ledger.credit to amount\n  audit.emit (Moved from to amount clock.now)"
    with pytest.raises(mcp.ToolError, match="transfer is defined in .*ledger.k; pass that file"):
        mcp.kern_replace(str(root / "orders.k"), "transfer", weak)
    out = mcp.kern_replace(str(ledger), "transfer", weak)
    assert "need from != to" not in ledger.read_text()                   # written where it lives
    assert (root / "orders.k").read_text() == before_orders              # the other file untouched
    assert {"buy", "refund"} <= set(out["affected"])                     # callers in orders.k re-checked


def test_mcp_refuses_an_edit_that_breaks_an_import(tmp_path):
    root = _copy(tmp_path, "shop")
    before = (root / "inventory" / "promotions.k").read_text()
    bad = "discount_range lo:Int hi:Int\n  ok otherwise\nbody\n  transfer lo hi 1\n"
    with pytest.raises(mcp.ToolError, match="rejected, file unchanged"):
        mcp.kern_replace(str(root / "inventory" / "promotions.k"), "discount_range", bad)
    assert (root / "inventory" / "promotions.k").read_text() == before


def test_mcp_root_covers_every_file_of_the_package(tmp_path, monkeypatch):
    root = _copy(tmp_path, "shop")
    monkeypatch.setenv("KERN_MCP_ROOT", str(root / "money"))             # a root that holds only part of it
    with pytest.raises(mcp.ToolError, match="outside this server's root"):
        mcp.kern_list(str(root / "money" / "ledger.k"))


def test_the_host_serves_a_package():
    from kern.host import Host
    defs = package.load(os.path.join(ROOT, "examples/shop"))
    h = Host(defs, ":memory:", clock=lambda: 1000, sinks=[])
    h.init("A:100@u1 S:0@u2 price.7:30 stock.7:2")
    r = h.call("buy", ["A", "S", 7])
    assert r["status"] == "ok", r
    w = h.load()
    assert w.bal == {"A": 70, "S": 30} and w.maps["stock"][7] == 1


@pytest.mark.parametrize("app,world,call,check", [
    ("library", "M:20@u1 L:0@u9 copies.3:1", ("borrow", [9, 3]),
     lambda w: w.maps["copies"][3] == 0 and w.maps["due"][9] == 1014),
    ("lottery", "B:25@u1 P:0@u0 A:0@u2 C:0@u3 D:0@u4", ("buy_ticket", [4, "B", "P"]),
     lambda w: w.bal["B"] == 15 and w.bal["P"] == 10 and w.maps["tickets"][4] == 1),
])
def test_the_host_serves_the_other_packages(app, world, call, check):
    from kern.host import Host
    h = Host(package.load(os.path.join(ROOT, "examples", app)), ":memory:", clock=lambda: 1000, sinks=[])
    h.init(world)
    r = h.call(*call)
    assert r["status"] == "ok", r
    assert check(h.load())


def test_the_host_draws_through_the_lottery_package_and_replays_it():
    from kern.host import Host
    h = Host(package.load(os.path.join(ROOT, "examples/lottery")), ":memory:", clock=lambda: 1000, sinks=[])
    h.init("P:100@u0 A:0@u1 B:0@u2 C:0@u3")
    r = h.call("draw", ["P", "A", "B", "C", 30])
    assert r["status"] == "ok", r
    w = h.load()
    assert w.bal["P"] == 70 and sorted(w.bal[x] for x in "ABC") == [0, 0, 30]
    assert h.replay() == []                                   # the draw was recorded and replays exactly
