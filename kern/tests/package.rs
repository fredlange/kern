//! Packages and imports (port of tests/test_package.py): the loader, its errors, the tools across files, the host.

mod common;
use common::{copy_example, kern, tmp, write_tree, ROOT};
use indexmap::IndexMap;
use kern::host::{Host, Options};
use kern::package;
use kern::run::Value;
use serde_json::{json, Value as J};

fn ledger() -> String {
    std::fs::read_to_string(format!("{ROOT}/examples/shop/money/ledger.k")).unwrap()
}

const PAY: &str = "import money.ledger

pay a:Account b:Account
  need as transfer a b 5
  ok otherwise
  keep ledger.total
  emit as transfer a b 5
  eg A:10@u1 B:0@u2 | pay A B -> ok | A:5 B:5
body
  transfer a b 5
";

fn load(p: &std::path::Path) -> Result<kern::syntax::Module, String> {
    package::load(&p.to_string_lossy(), &IndexMap::new()).map_err(|e| e.0)
}

fn all_clean(defs: &kern::syntax::Module) -> bool {
    kern::report::analyse(defs, false, None, None).unwrap().values().all(|r| r.clean)
}

// ---- condition 1: the loader

#[test]
fn every_example_package_loads_and_is_clean() {
    for app in ["shop", "library", "lottery"] {
        let defs = package::load(&format!("{ROOT}/examples/{app}"), &IndexMap::new()).unwrap();
        let files = package::files_of(&defs);
        assert!(files.len() >= 3 && files.values().all(|f| std::path::Path::new(f).exists()), "{app}");
        assert!(defs.defs.values().all(|d| d.file.as_ref().map_or(false, |f| files.values().any(|x| x == f))), "{app}");
        assert!(all_clean(&defs), "{app}");
    }
}

#[test]
fn any_file_or_the_directory_loads_the_same_package() {
    let mut names = std::collections::BTreeSet::new();
    for f in ["orders.k", "money/ledger.k", "inventory/catalog.k", ".", "kern.pkg"] {
        let defs = package::load(&format!("{ROOT}/examples/shop/{f}"), &IndexMap::new()).unwrap();
        let mut n: Vec<String> = defs.defs.keys().cloned().collect();
        n.sort();
        names.insert(n);
    }
    assert_eq!(names.len(), 1);
}

#[test]
fn a_name_used_but_not_imported_is_an_error() {
    let root = write_tree(&tmp("pkg1").join("p"), &[("money/ledger.k", &ledger()), ("pay.k", &PAY.replace("import money.ledger\n", ""))], true);
    let e = load(&root.join("pay.k")).unwrap_err();
    let re = regex::Regex::new(r"pay.k:\d+: pay uses def transfer, which money.ledger declares: add `import money.ledger`").unwrap();
    assert!(re.is_match(&e), "{e}");
}

#[test]
fn state_from_a_file_not_imported_is_an_error() {
    let cat = "state stock Int\n\nrestock item:Int\n  ok otherwise\nbody\n  stock.set item 1\n";
    let sell = "sell item:Int\n  ok otherwise\nbody\n  stock.set item 0\n";
    let root = write_tree(&tmp("pkg2").join("p"), &[("catalog.k", cat), ("sell.k", sell)], true);
    let e = load(&root.join("sell.k")).unwrap_err();
    assert!(e.contains("uses state stock, which catalog declares"), "{e}");
}

#[test]
fn an_unknown_module_is_an_error_with_file_and_line() {
    let root = write_tree(&tmp("pkg3").join("p"), &[("pay.k", &PAY.replace("import money.ledger", "import money.ledgr"))], true);
    let e = load(&root.join("pay.k")).unwrap_err();
    assert!(e.contains("pay.k:1: no module money.ledgr (looked for money/ledgr.k)"), "{e}");
}

#[test]
fn a_duplicate_name_across_files_is_an_error() {
    let root = write_tree(&tmp("pkg4").join("p"), &[("a.k", &ledger()), ("b.k", &ledger())], true);
    let e = load(&root.join("a.k")).unwrap_err();
    assert!(e.contains("transfer is already defined in a"), "{e}");
}

#[test]
fn an_import_can_only_name_a_module() {
    // isolation: module names are dotted identifiers, so nothing outside the package can be named
    for (i, bad) in ["import ../ledger", "import /etc/passwd", "import money..ledger", "import Money"].iter().enumerate() {
        let root = write_tree(&tmp(&format!("pkg5{i}")).join("p"), &[("money/ledger.k", &ledger()), ("pay.k", &PAY.replace("import money.ledger", bad))], true);
        let e = load(&root.join("pay.k")).unwrap_err();
        assert!(e.contains("want `import a.b`"), "{bad}: {e}");
    }
}

#[test]
fn outside_a_package_a_file_loads_what_it_imports() {
    let root = write_tree(&tmp("pkg6").join("loose"), &[("money/ledger.k", &ledger()), ("pay.k", PAY)], false);
    let defs = load(&root.join("pay.k")).unwrap();
    let mut names: Vec<&String> = defs.defs.keys().collect();
    names.sort();
    assert_eq!(names, ["pay", "transfer"]);
    assert!(all_clean(&defs));
}

#[test]
fn a_single_file_without_imports_is_loaded_exactly_as_before() {
    let path = format!("{ROOT}/examples/escrow/escrow_s1.k");
    let a = package::load(&path, &IndexMap::new()).unwrap();
    let b = kern::syntax::parse_module(&std::fs::read_to_string(&path).unwrap(), true).unwrap();
    kern::wf::check_module(&b).unwrap();
    assert_eq!(a.defs.values().map(|d| d.hash()).collect::<Vec<_>>(), b.defs.values().map(|d| d.hash()).collect::<Vec<_>>());
    assert!(a.package.is_none());
}

#[test]
fn import_in_a_single_file_parse_says_to_use_the_loader() {
    let e = kern::syntax::parse_module(PAY, true).unwrap_err();
    assert!(e.0.contains("needs the package loader"), "{e}");
}

// ---- condition 2: the tools

#[test]
fn cli_check_on_any_file_checks_the_package_and_names_the_files() {
    let (code, out, err) = kern(&["check", "examples/library/fines.k", "--quick"], ROOT);
    assert_eq!(code, 0, "{out}{err}");
    for m in ["== catalog.books", "== loans", "== fines", "== money.ledger"] {
        assert!(out.contains(m), "{m}");
    }
    let (code, out, _) = kern(&["view", "examples/library/fines.k", "borrow", "--quick"], ROOT); // defined in loans.k
    assert_eq!(code, 0);
    assert!(out.lines().next().unwrap().starts_with("borrow loan:Int book:Int"));
}

fn tool(name: &str, args: J) -> Result<J, String> {
    match kern::mcp::call_tool(name, args.as_object().unwrap()).unwrap() {
        Ok(v) => Ok(v),
        Err(kern::mcp::ToolError(k, m)) => Err(format!("{k}: {m}")),
    }
}

#[test]
fn mcp_lists_views_and_checks_a_package() {
    let root = copy_example("pkg7", "shop");
    let orders = root.join("orders.k").to_string_lossy().into_owned();
    let list = tool("kern_list", json!({"file": orders})).unwrap();
    let names: std::collections::BTreeSet<&str> =
        list["definitions"].as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["buy", "refund", "transfer", "set_price", "restock", "restock_range", "discount_range"].into_iter().collect());
    let v = tool("kern_view", json!({"file": orders, "name": "transfer", "as": "iface"})).unwrap();
    assert!(v["text"].as_str().unwrap().contains("transfer from:Account"));
}

#[test]
fn mcp_replace_writes_the_defining_file_and_rechecks_callers_in_other_files() {
    let root = copy_example("pkg8", "shop");
    let ledger_path = root.join("money").join("ledger.k");
    let before_orders = std::fs::read_to_string(root.join("orders.k")).unwrap();
    let weak = ledger().split("body\n").next().unwrap().replace("  need from != to else SameAccount\n", "")
        + "body\n  ledger.debit from amount\n  ledger.credit to amount\n  audit.emit (Moved from to amount clock.now)";
    let e = tool("kern_replace", json!({"file": root.join("orders.k").to_string_lossy(), "name": "transfer", "source": weak})).unwrap_err();
    assert!(regex::Regex::new("transfer is defined in .*ledger.k; pass that file").unwrap().is_match(&e), "{e}");
    let out = tool("kern_replace", json!({"file": ledger_path.to_string_lossy(), "name": "transfer", "source": weak})).unwrap();
    assert!(!std::fs::read_to_string(&ledger_path).unwrap().contains("need from != to")); // written where it lives
    assert_eq!(std::fs::read_to_string(root.join("orders.k")).unwrap(), before_orders); // the other file untouched
    let affected = out["affected"].as_object().unwrap();
    assert!(affected.contains_key("buy") && affected.contains_key("refund")); // callers in orders.k re-checked
}

#[test]
fn mcp_refuses_an_edit_that_breaks_an_import() {
    let root = copy_example("pkg9", "shop");
    let promo = root.join("inventory").join("promotions.k");
    let before = std::fs::read_to_string(&promo).unwrap();
    let bad = "discount_range lo:Int hi:Int\n  ok otherwise\nbody\n  transfer lo hi 1\n";
    let e = tool("kern_replace", json!({"file": promo.to_string_lossy(), "name": "discount_range", "source": bad})).unwrap_err();
    assert!(e.contains("rejected, file unchanged"), "{e}");
    assert_eq!(std::fs::read_to_string(&promo).unwrap(), before);
}

#[test]
fn mcp_root_covers_every_file_of_the_package() {
    // a root that holds only part of the package; run in a child process, since the root is an env variable
    let root = copy_example("pkg10", "shop");
    let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kern_list","arguments":{"file":"LEDGER"}}}"#
        .replace("LEDGER", &root.join("money/ledger.k").to_string_lossy());
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_kern"))
        .arg("mcp")
        .env("KERN_MCP_ROOT", root.join("money"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    writeln!(child.stdin.take().unwrap(), "{msg}").unwrap();
    let out = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    assert!(out.contains("outside this server's root"), "{out}");
}

fn host<'m>(defs: &'m kern::syntax::Module) -> Host<'m> {
    let reports = kern::report::analyse(defs, false, None, None).unwrap();
    Host::new(defs, reports, Options { clock: Some(Box::new(|| 1000)), sinks: Some(vec![]), ..Default::default() }).unwrap()
}

#[test]
fn the_host_serves_a_package() {
    let defs = package::load(&format!("{ROOT}/examples/shop"), &IndexMap::new()).unwrap();
    let mut h = host(&defs);
    h.init("A:100@u1 S:0@u2 price.7:30 stock.7:2").unwrap();
    let r = h.call("buy", &[Value::Str("A".into()), Value::Str("S".into()), Value::Int(7)]).unwrap();
    assert_eq!(r["status"], "ok", "{r:?}");
    let w = h.load().unwrap();
    assert_eq!((w.bal["A"], w.bal["S"]), (70, 30));
    assert_eq!(w.maps["stock"][&kern::syntax::Key::Int(7)], 1);
}

#[test]
fn the_host_serves_the_other_packages() {
    let defs = package::load(&format!("{ROOT}/examples/library"), &IndexMap::new()).unwrap();
    let mut h = host(&defs);
    h.init("M:20@u1 L:0@u9 copies.3:1").unwrap();
    let r = h.call("borrow", &[Value::Int(9), Value::Int(3)]).unwrap();
    assert_eq!(r["status"], "ok", "{r:?}");
    let w = h.load().unwrap();
    assert_eq!(w.maps["copies"][&kern::syntax::Key::Int(3)], 0);
    assert_eq!(w.maps["due"][&kern::syntax::Key::Int(9)], 1014);

    let defs = package::load(&format!("{ROOT}/examples/lottery"), &IndexMap::new()).unwrap();
    let mut h = host(&defs);
    h.init("B:25@u1 P:0@u0 A:0@u2 C:0@u3 D:0@u4").unwrap();
    let r = h.call("buy_ticket", &[Value::Int(4), Value::Str("B".into()), Value::Str("P".into())]).unwrap();
    assert_eq!(r["status"], "ok", "{r:?}");
    let w = h.load().unwrap();
    assert_eq!((w.bal["B"], w.bal["P"]), (15, 10));
    assert_eq!(w.maps["tickets"][&kern::syntax::Key::Int(4)], 1);
}

#[test]
fn the_host_draws_through_the_lottery_package_and_replays_it() {
    let defs = package::load(&format!("{ROOT}/examples/lottery"), &IndexMap::new()).unwrap();
    let mut h = host(&defs);
    h.init("P:100@u0 A:0@u1 B:0@u2 C:0@u3").unwrap();
    let args: Vec<Value> = ["P", "A", "B", "C"].iter().map(|a| Value::Str(a.to_string())).chain([Value::Int(30)]).collect();
    let r = h.call("draw", &args).unwrap();
    assert_eq!(r["status"], "ok", "{r:?}");
    let w = h.load().unwrap();
    assert_eq!(w.bal["P"], 70);
    let mut won: Vec<i128> = ["A", "B", "C"].iter().map(|x| w.bal[*x]).collect();
    won.sort();
    assert_eq!(won, [0, 0, 30]);
    assert!(h.replay().unwrap().is_empty()); // the draw was recorded and replays exactly
}
