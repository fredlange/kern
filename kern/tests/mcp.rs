//! The MCP server (port of tests/test_mcp.py): docs/research/ROADMAP_RUN.md, S3, pass conditions 1-14 (15 is the
//! real-client smoke test, run by hand and recorded there).

mod common;
use common::{tmp, ROOT};
use serde_json::{json, Value as J};
use std::io::{BufRead, BufReader, Write};

fn escrow() -> String {
    std::fs::read_to_string(format!("{ROOT}/examples/escrow/escrow_s1.k")).unwrap()
}

fn module(name: &str) -> String {
    let p = tmp(name).join("escrow_s1.k");
    std::fs::write(&p, escrow()).unwrap();
    p.to_string_lossy().into_owned()
}

fn tool(name: &str, args: J) -> Result<J, String> {
    match kern::mcp::call_tool(name, args.as_object().unwrap()).expect("a tool") {
        Ok(v) => Ok(v),
        Err(kern::mcp::ToolError(k, m)) => Err(format!("{k}: {m}")),
    }
}

/// A scripted MCP client over stdio, as an agent harness would run it.
struct Client {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    n: i64,
}

impl Client {
    fn new(env: &[(&str, &str)]) -> Client {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_kern"));
        cmd.arg("mcp").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Client { child, out, n: 0 }
    }
    fn send(&mut self, method: &str, params: J, notify: bool) -> Option<J> {
        let mut msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if !notify {
            self.n += 1;
            msg["id"] = json!(self.n);
        }
        writeln!(self.child.stdin.as_mut().unwrap(), "{msg}").unwrap();
        if notify {
            return None;
        }
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let resp: J = serde_json::from_str(&line).unwrap();
        assert!(resp["jsonrpc"] == "2.0" && resp["id"] == json!(self.n));
        Some(resp)
    }
    fn tool(&mut self, name: &str, args: J) -> (J, bool) {
        let r = self.send("tools/call", json!({"name": name, "arguments": args}), false).unwrap()["result"].clone();
        let text = r["content"][0]["text"].as_str().unwrap().to_string();
        let err = r["isError"].as_bool().unwrap();
        (if err { json!(text) } else { serde_json::from_str(&text).unwrap() }, err)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn c1_protocol_and_every_tool() {
    let m = module("mcp1");
    let mut c = Client::new(&[]);
    let init = c.send("initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t"}}), false).unwrap();
    assert_eq!(init["result"]["capabilities"], json!({"tools": {}}));
    c.send("notifications/initialized", json!({}), true); // no response expected
    let names: std::collections::BTreeSet<String> =
        c.send("tools/list", json!({}), false).unwrap()["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names.len(), 10);
    assert!(!c.tool("kern_list", json!({"file": m})).1);
    assert!(!c.tool("kern_view", json!({"file": m, "name": "capture", "as": "spec"})).1);
    assert!(!c.tool("kern_why", json!({"file": m, "name": "capture"})).1);
    assert_eq!(c.tool("kern_check", json!({"file": m})).0["clean"], true);
    assert_eq!(c.tool("kern_run", json!({"file": m, "call": "transfer A B 100", "world": "A:500@u1 B:0@u2"})).0["outcome"], "ok");
    assert_eq!(c.tool("kern_callers", json!({"file": m, "name": "move"})).0["direct"], json!(["capture", "refund", "split", "tip"]));
    assert_eq!(c.send("nope", json!({}), false).unwrap()["error"]["code"], -32601);
    assert!(c.tool("kern_view", json!({"file": m, "name": "ghost"})).1); // a tool error, not a crash
}

#[test]
fn c2_views_are_identical_to_the_cli() {
    let m = module("mcp2");
    for name in ["transfer", "capture", "refund"] {
        for as_ in ["iface", "spec", "editor"] {
            let (_, cli, _) = common::kern(&["view", &m, name, "--as", as_], ROOT);
            let v = tool("kern_view", json!({"file": m, "name": name, "as": as_})).unwrap();
            assert_eq!(format!("{}\n", v["text"].as_str().unwrap()), cli);
        }
    }
}

fn transfer_broken() -> String {
    escrow().split("\n\nmove ").next().unwrap().replace("  ledger.credit to amount\n", "  ledger.credit to (amount - 1)\n")
}

fn transfer_same() -> String {
    escrow().split("\n\nmove ").next().unwrap().replace(
        "  ledger.debit from amount\n  ledger.credit to amount\n",
        "  ledger.credit to amount\n  ledger.debit from amount\n",
    )
}

fn names(v: &J) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

#[test]
fn c3_breaking_edit_names_the_broken_callers_and_equivalent_edit_is_ok() {
    let m = module("mcp3");
    let r = tool("kern_replace", json!({"file": m, "name": "transfer", "source": transfer_broken()})).unwrap();
    let broken = names(&r["broken"]);
    assert!(r["ok"] == false && broken.contains(&"transfer".into()) && broken.contains(&"capture".into())); // callers re-checked
    std::fs::write(&m, escrow()).unwrap();
    let r = tool("kern_replace", json!({"file": m, "name": "transfer", "source": transfer_same()})).unwrap();
    assert_eq!(r["ok"], true);
    let after = std::fs::read_to_string(&m).unwrap();
    assert_eq!(after.split_once("\n\nmove ").unwrap().1, escrow().split_once("\n\nmove ").unwrap().1); // rest untouched
}

#[test]
fn c4_ill_formed_edit_leaves_the_file_unchanged() {
    let m = module("mcp4");
    let before = std::fs::read_to_string(&m).unwrap();
    for bad in ["transfer from:Account\nbody\n  ledger.debit from nothing\n", "transfer from:Account\n  need x\nbody\n"] {
        assert!(tool("kern_replace", json!({"file": m, "name": "transfer", "source": bad})).is_err());
        assert_eq!(std::fs::read_to_string(&m).unwrap(), before);
    }
}

#[test]
fn c5_recheck_only_reproves_the_edited_definition_and_its_callers() {
    let m = module("mcp5");
    tool("kern_check", json!({"file": m})).unwrap(); // warm the cache
    let e = escrow();
    let split = format!("split {}", e.split_once("\n\nsplit ").unwrap().1.split_once("\n\ntip ").unwrap().0);
    let swapped = split.replace("  move escrow a part\n  move escrow b (amount - part)", "  move escrow b (amount - part)\n  move escrow a part");
    assert_ne!(swapped, split);
    let r = tool("kern_replace", json!({"file": m, "name": "split", "source": swapped})).unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(names(&r["rechecked"]), ["split"]); // split has no callers
}

#[test]
fn c7_end_to_end_task_through_the_server_only() {
    // add `refund_all` (release the whole hold) using only views, add and check
    let m = module("mcp7");
    let spec = tool("kern_view", json!({"file": m, "name": "release", "as": "spec"})).unwrap();
    assert!(spec["text"].as_str().unwrap().contains("need ledger.balance escrow >= amount"));
    let new = "refund_all escrow:Account buyer:Account amount:Int
  need as release escrow buyer amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer + amount
  emit as release escrow buyer amount
  eg A:0@u1 E:100@u0 | refund_all E A 100 -> ok | A:100 E:0
body
  release escrow buyer amount
";
    let r = tool("kern_add", json!({"file": m, "source": new})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(tool("kern_check", json!({"file": m})).unwrap()["status"]["refund_all"], "All guarantees proven");
}

#[test]
fn c8_kern_edit_is_atomic() {
    let m = module("mcp8");
    let before = std::fs::read_to_string(&m).unwrap();
    let ops = json!([{"op": "replace", "name": "transfer", "source": transfer_same()},
                     {"op": "add", "source": "bad x:Int\nbody\n  ledger.credit x 1\n"}]);
    assert!(tool("kern_edit", json!({"file": m, "ops": ops})).is_err());
    assert_eq!(std::fs::read_to_string(&m).unwrap(), before);
    let ops = json!([{"op": "replace", "name": "transfer", "source": transfer_same()},
                     {"op": "add", "source": "noop a:Account\n  ok otherwise\n  keep ledger.total\nbody\n  ledger.credit a 0\n"}]);
    let r = tool("kern_edit", json!({"file": m, "ops": ops})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
}

#[test]
fn c9_views_show_a_red_callee_as_not_proven() {
    let m = module("mcp9");
    tool("kern_replace", json!({"file": m, "name": "transfer", "source": transfer_broken()})).unwrap();
    let ed = tool("kern_view", json!({"file": m, "name": "reserve", "as": "editor"})).unwrap();
    let part = ed["text"].as_str().unwrap().split_once("\n\ntransfer ").unwrap().1.split("\n\n").next().unwrap().to_string();
    assert!(!part.contains("proof ok") && part.contains("open"));
}

#[test]
fn c10_edits_report_stale_and_orphaned_why_entries() {
    let d = tmp("mcp10");
    let v7 = std::fs::read_to_string(format!("{ROOT}/examples/ledger-tour/ledger_v7.k")).unwrap();
    std::fs::write(d.join("m.k"), &v7).unwrap();
    std::fs::copy(format!("{ROOT}/examples/ledger-tour/ledger_v7.why"), d.join("m.why")).unwrap();
    let p = d.join("m.k").to_string_lossy().into_owned();
    let payout = format!("payout {}", v7.split_once("\n\npayout ").unwrap().1);
    let r = tool("kern_replace", json!({"file": p, "name": "payout",
        "source": payout.replace("  eg A:500@u1", "  eg A:600@u1").replace("-> ok | A:395", "-> ok | A:495")})).unwrap();
    assert!(!names(&r["why_now_stale"]).contains(&"payout".into())); // examples don't stale a why
    let r = tool("kern_replace", json!({"file": p, "name": "payout",
        "source": payout.replace("  transfer from house fee\n", "  transfer from house fee\n  ledger.credit to 0\n")})).unwrap();
    assert!(names(&r["why_now_stale"]).contains(&"payout".into()));
    let r = tool("kern_remove", json!({"file": p, "name": "payout"})).unwrap();
    assert_eq!(names(&r["why_orphaned"]), ["payout"]);
}

#[test]
fn c11_callers() {
    let m = module("mcp11");
    let r = tool("kern_callers", json!({"file": m, "name": "transfer"})).unwrap();
    let mut want = vec!["move", "reserve", "release", "capture", "purchase", "cancel", "refund", "split", "tip"];
    want.sort();
    assert_eq!(names(&r["transitive"]), want);
}

#[test]
fn c12_token_log() {
    let m = module("mcp12");
    let log = tmp("mcp12log").join("log.jsonl");
    {
        let mut c = Client::new(&[("KERN_MCP_LOG", &log.to_string_lossy())]);
        c.send("initialize", json!({}), false);
        c.tool("kern_view", json!({"file": m, "name": "capture", "as": "editor"}));
    }
    let rows: Vec<J> = std::fs::read_to_string(&log).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(rows[0]["tool"], "kern_view");
    assert!(rows[0]["response_chars"].as_u64().unwrap() > 0);
}

#[test]
fn c13_check_summary_is_small_and_detail_on_request() {
    let m = module("mcp13");
    let s = tool("kern_check", json!({"file": m})).unwrap();
    assert!(s["clean"] == true && s["problems"] == "" && s["status"].as_object().unwrap().len() == 10);
    let d = tool("kern_check", json!({"file": m, "name": "capture"})).unwrap();
    assert!(d["text"].as_str().unwrap().contains("✔ ok otherwise"));
}

#[test]
fn c14_expect_hash() {
    let m = module("mcp14");
    let real = kern::syntax::parse_module(&escrow(), true).unwrap().defs["transfer"].hash();
    let e = tool("kern_replace", json!({"file": m, "name": "transfer", "source": transfer_same(), "expect_hash": "00000000"})).unwrap_err();
    assert!(e.contains("changed underneath"), "{e}");
    let r = tool("kern_replace", json!({"file": m, "name": "transfer", "source": transfer_same(), "expect_hash": real})).unwrap();
    assert_eq!(r["ok"], true);
}

#[test]
fn root_keeps_the_server_inside_the_workspace() {
    // C2 isolation (kern-90): with KERN_MCP_ROOT set, no tool opens a module outside it
    let m = module("mcp15");
    let dir = std::path::Path::new(&m).parent().unwrap().to_string_lossy().into_owned();
    let mut c = Client::new(&[("KERN_MCP_ROOT", &dir)]);
    let (list, err) = c.tool("kern_list", json!({"file": m}));
    assert!(!err && !list["definitions"].as_array().unwrap().is_empty());
    let (msg, err) = c.tool("kern_view", json!({"file": format!("{ROOT}/examples/escrow/escrow_s2.k"), "name": "capture"}));
    assert!(err && msg.as_str().unwrap().contains("outside this server's root"), "{msg}");
}
