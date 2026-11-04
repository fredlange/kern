"""The MCP server (kern/mcp.py): docs/research/ROADMAP_RUN.md, S3, pass conditions 1-14 (15 is the real-client
smoke test, run by hand and recorded there)."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys

import pytest

from kern import mcp

ESCROW = open("examples/escrow_s1.k").read()


@pytest.fixture
def module(tmp_path):
    p = tmp_path / "escrow_s1.k"
    p.write_text(ESCROW)
    return str(p)


class Client:
    """A scripted MCP client over stdio, as an agent harness would run it."""

    def __init__(self, env=None):
        self.p = subprocess.Popen([sys.executable, "-m", "kern.mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  text=True, cwd=os.getcwd(), env=dict(os.environ, **(env or {})))
        self.n = 0

    def send(self, method, params=None, notify=False):
        msg = {"jsonrpc": "2.0", "method": method, "params": params or {}}
        if not notify:
            self.n += 1
            msg["id"] = self.n
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()
        if notify:
            return None
        resp = json.loads(self.p.stdout.readline())
        assert resp["jsonrpc"] == "2.0" and resp["id"] == self.n
        return resp

    def tool(self, tool_name, **args):
        r = self.send("tools/call", {"name": tool_name, "arguments": args})["result"]
        text = r["content"][0]["text"]
        return (json.loads(text) if not r["isError"] else text), r["isError"]

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)


def test_c1_protocol_and_every_tool(module):
    c = Client()
    try:
        init = c.send("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t"}})
        assert init["result"]["capabilities"] == {"tools": {}}
        c.send("notifications/initialized", notify=True)                  # no response expected
        names = {t["name"] for t in c.send("tools/list")["result"]["tools"]}
        assert names == set(mcp.HANDLERS)
        assert not c.tool("kern_list", file=module)[1]
        assert not c.tool("kern_view", file=module, name="capture", **{"as": "spec"})[1]
        assert not c.tool("kern_why", file=module, name="capture")[1]
        assert c.tool("kern_check", file=module)[0]["clean"]
        assert c.tool("kern_run", file=module, call="transfer A B 100", world="A:500@u1 B:0@u2")[0]["outcome"] == "ok"
        assert c.tool("kern_callers", file=module, name="move")[0]["direct"] == ["capture", "refund", "split", "tip"]
        assert c.send("nope")["error"]["code"] == -32601
        assert c.tool("kern_view", file=module, name="ghost")[1]                 # a tool error, not a crash
    finally:
        c.close()


def test_c2_views_are_identical_to_the_cli(module):
    for name in ("transfer", "capture", "refund"):
        for as_ in ("iface", "spec", "editor"):
            cli = subprocess.run([sys.executable, "-m", "kern", "view", module, name, "--as", as_],
                                 capture_output=True, text=True).stdout
            assert mcp.kern_view(module, name, **{"as": as_})["text"] + "\n" == cli


TRANSFER_BROKEN = ESCROW.split("\n\nmove ")[0].replace("  ledger.credit to amount\n", "  ledger.credit to (amount - 1)\n")
TRANSFER_SAME = ESCROW.split("\n\nmove ")[0].replace("  ledger.debit from amount\n  ledger.credit to amount\n",
                                                     "  ledger.credit to amount\n  ledger.debit from amount\n")


def test_c3_breaking_edit_names_the_broken_callers_and_equivalent_edit_is_ok(module):
    r = mcp.kern_replace(module, "transfer", TRANSFER_BROKEN)
    assert not r["ok"] and "transfer" in r["broken"] and "capture" in r["broken"]      # callers re-checked
    open(module, "w").write(ESCROW)
    r = mcp.kern_replace(module, "transfer", TRANSFER_SAME)
    assert r["ok"]
    after = open(module).read()
    assert after.split("\n\nmove ", 1)[1] == ESCROW.split("\n\nmove ", 1)[1]          # rest untouched


def test_c4_ill_formed_edit_leaves_the_file_unchanged(module):
    before = open(module).read()
    for bad in ("transfer from:Account\nbody\n  ledger.debit from nothing\n", "transfer from:Account\n  need x\nbody\n"):
        with pytest.raises(mcp.ToolError):
            mcp.kern_replace(module, "transfer", bad)
        assert open(module).read() == before


def test_c5_recheck_only_reproves_the_edited_definition_and_its_callers(module):
    mcp.kern_check(module)                                                  # warm the cache
    split = "split " + ESCROW.split("\n\nsplit ", 1)[1].split("\n\ntip ", 1)[0]
    swapped = split.replace("  move escrow a part\n  move escrow b (amount - part)",
                            "  move escrow b (amount - part)\n  move escrow a part")
    assert swapped != split
    r = mcp.kern_replace(module, "split", swapped)
    assert r["ok"] and set(r["rechecked"]) == {"split"}                      # split has no callers


def test_c7_end_to_end_task_through_the_server_only(module):
    """Add `refund_all` (release the whole hold) using only views, add and check."""
    spec_release = mcp.kern_view(module, "release", **{"as": "spec"})["text"]
    assert "need ledger.balance escrow >= amount" in spec_release
    new = """refund_all escrow:Account buyer:Account amount:Int
  need as release escrow buyer amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer + amount
  emit as release escrow buyer amount
  eg A:0@u1 E:100@u0 | refund_all E A 100 -> ok | A:100 E:0
body
  release escrow buyer amount
"""
    r = mcp.kern_add(module, new)
    assert r["ok"], r
    assert mcp.kern_check(module)["status"]["refund_all"] == "All guarantees proven"


def test_c8_kern_edit_is_atomic(module):
    before = open(module).read()
    with pytest.raises(mcp.ToolError):
        mcp.kern_edit(module, [{"op": "replace", "name": "transfer", "source": TRANSFER_SAME},
                               {"op": "add", "source": "bad x:Int\nbody\n  ledger.credit x 1\n"}])
    assert open(module).read() == before
    r = mcp.kern_edit(module, [{"op": "replace", "name": "transfer", "source": TRANSFER_SAME},
                               {"op": "add", "source": "noop a:Account\n  ok otherwise\n  keep ledger.total\nbody\n  ledger.credit a 0\n"}])
    assert r["ok"], r


def test_c9_views_show_a_red_callee_as_not_proven(module):
    mcp.kern_replace(module, "transfer", TRANSFER_BROKEN)
    ed = mcp.kern_view(module, "reserve", **{"as": "editor"})["text"]
    transfer_part = ed.split("\n\ntransfer ", 1)[1]
    assert "proof ok" not in transfer_part.split("\n\n")[0] and "open" in transfer_part.split("\n\n")[0]


def test_c10_edits_report_stale_and_orphaned_why_entries(tmp_path):
    p = tmp_path / "m.k"
    p.write_text(open("examples/ledger_v7.k").read())
    shutil.copy("examples/ledger_v7.why", tmp_path / "m.why")
    v7 = open("examples/ledger_v7.k").read()
    payout = "payout " + v7.split("\n\npayout ", 1)[1]
    r = mcp.kern_replace(str(p), "payout", payout.replace("  eg A:500@u1", "  eg A:600@u1").replace("-> ok | A:395", "-> ok | A:495"))
    assert r["why_now_stale"] == [] or "payout" not in r["why_now_stale"]   # examples don't stale a why
    r = mcp.kern_replace(str(p), "payout", payout.replace("  transfer from house fee\n", "  transfer from house fee\n  ledger.credit to 0\n"))
    assert "payout" in r["why_now_stale"]
    r = mcp.kern_remove(str(p), "payout")
    assert r["why_orphaned"] == ["payout"]


def test_c11_callers(module):
    assert mcp.kern_callers(module, "transfer")["transitive"] == sorted(
        ["move", "reserve", "release", "capture", "purchase", "cancel", "refund", "split", "tip"])


def test_c12_token_log(module, tmp_path):
    log = tmp_path / "log.jsonl"
    c = Client(env={"KERN_MCP_LOG": str(log)})
    try:
        c.send("initialize", {})
        c.tool("kern_view", file=module, name="capture", **{"as": "editor"})
    finally:
        c.close()
    rows = [json.loads(l) for l in log.read_text().splitlines()]
    assert rows[0]["tool"] == "kern_view" and rows[0]["response_chars"] > 0


def test_c13_check_summary_is_small_and_detail_on_request(module):
    s = mcp.kern_check(module)
    assert s["clean"] and s["problems"] == "" and len(s["status"]) == 10
    assert "✔ ok otherwise" in mcp.kern_check(module, name="capture")["text"]


def test_c14_expect_hash(module):
    h = mcp.kern_list(module)["definitions"][0]["iface"]
    from kern.syntax import parse_module
    real = parse_module(ESCROW)["transfer"].hash
    with pytest.raises(mcp.ToolError, match="changed underneath"):
        mcp.kern_replace(module, "transfer", TRANSFER_SAME, expect_hash="00000000")
    assert mcp.kern_replace(module, "transfer", TRANSFER_SAME, expect_hash=real)["ok"]
    assert h


def test_root_keeps_the_server_inside_the_workspace(module, monkeypatch):
    """C2 isolation (kern-90): with KERN_MCP_ROOT set, no tool opens a module outside it."""
    monkeypatch.setenv("KERN_MCP_ROOT", os.path.dirname(module))
    assert mcp.kern_list(module)["definitions"]
    with pytest.raises(mcp.ToolError, match="outside this server's root"):
        mcp.kern_view(os.path.abspath("examples/escrow_s2.k"), "capture")
