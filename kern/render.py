"""Proof page (HTML) and terminal summary, rendered from verifier results with fixed
templates. Nothing here is written by a language model, so the page cannot say more
than the checks established."""
from __future__ import annotations

import html

from .infer import fails as inferred_fails
from .views import EFFECTS, en, en_emits, en_ensure, en_refusal, lets_of

ICON = {"proven": "✔", "refuted": "✗", "conditional": "!", "unknown": "?"}


def _status(r):
    gs = r["guarantees"]
    if any(g["status"] == "refuted" for g in gs) or r["props"]["violations"] or not all(e["pass"] for e in r["examples"]):
        return "fail", "Broken guarantees"
    if not gs:
        return "warn", "No guarantees stated"
    if any(g["status"] == "conditional" for g in gs):
        return "warn", "Depends on unproven code"
    if any(g["status"] != "proven" for g in gs):
        return "warn", "Not proven: prover gave no answer"
    return "ok", "All guarantees proven"


def _guarantee_text(g, d):
    kind, e = g["kind"], g["expr"]
    if kind == "emit":
        return en_emits(e)
    return f"{en(e)} is unchanged" if kind == "keep" else en_ensure(e)


def _how(g):
    if g["status"] == "proven":
        return "Proven for all inputs (Z3)."
    if g["status"] == "conditional":
        who = ", ".join(f"<code>{c}</code>" for c in g["blocked_by"])
        if g.get("via_broken_spec"):
            return (f"Unknown: it fails according to the spec of {who}, but that spec is itself broken, "
                    f"and the interpreter did not reproduce the failure.")
        return f"Proven only if the guarantees of {who} hold — and they currently do not."
    if g["status"] == "refuted":
        cx = g["counterexample"]
        conf = (" Replayed in the interpreter: confirmed." if cx["confirmed_by_interpreter"]
                else " Not reproduced in the interpreter.")
        got = f" The audit log received: {html.escape(cx['emitted'])}." if g["kind"] == "emit" else ""
        return f"Fails for this input:<pre>{html.escape(cx['eg'])}</pre>{got}{conf}"
    if g.get("reason"):
        return f"Cannot be proven: {html.escape(g['reason'])}."
    return "Prover gave no answer within the time limit."


def card(r, why):
    d = r["def"]
    cls, label = _status(r)
    lets = lets_of(d)
    out = [f'<section class="card"><header><h2><code>{d.name}</code></h2>'
           f'<span class="badge {cls}">{label}</span><span class="hash">build {r["hash"]}</span></header>']

    eff = ", ".join(EFFECTS.get(e, e) for e in sorted(r["uses"])) or "nothing"
    out.append(f'<h3>Can touch</h3><p>{eff}. <span class="muted">Inferred from the body; nothing else is reachable.</span></p>')

    rows = []
    for n in d.needs + [s for s in d.body if s[0] == "need"]:
        cond, _ = en_refusal(n, lets)
        detail = f" <span class='muted'>with</span> {', '.join(en(a, lets) for a in n[3])}" if n[3] else ""
        rows.append((cond, f"<code>{html.escape(n[2])}</code>{detail}"))
    own = {n[2] for n in d.needs + d.body if n[0] == "need"}
    for c in r["callees"]:
        inherited = [k for k in inferred_fails(r["defs"], r["defs"][c]) if k not in own]
        if inherited:
            rows.append((f"<code>{c}</code> refuses",
                         ", ".join(f"<code>{k}</code>" for k in inherited) + " <span class='muted'>(passed through)</span>"))
    if rows:
        out.append("<h3>Refuses when</h3><table>" + "".join(
            f"<tr><td>{cond}</td><td class='fail-name'>→ {f}</td></tr>" for cond, f in rows)
            + "</table><p class='muted'>A refusal undoes everything the call did.</p>")

    out.append("<h3>Guarantees</h3><ul class='g'>")
    for g in r["guarantees"]:
        out.append(f"<li class='{g['status']}'><span class='ic'>{ICON[g['status']]}</span>"
                   f"<div><div>{_guarantee_text(g, d)}</div><div class='how'>{_how(g)}</div>"
                   f"<div class='src'><code>{html.escape(g['kind'] + ' ' + g['clause'])}</code></div></div></li>")
    out.append("</ul>")

    ex = r["examples"]
    passed = sum(e["pass"] for e in ex)
    out.append(f"<h3>Examples <span class='muted'>{passed}/{len(ex)} pass, executed against build {r['hash']}</span></h3><ul class='ex'>")
    for e in ex:
        mark = "✔" if e["pass"] else f"✗ got {html.escape(e['got'])}"
        out.append(f"<li><code>{html.escape(e['src'])}</code> <span class='{'okc' if e['pass'] else 'failc'}'>{mark}</span></li>")
    out.append("</ul>")

    p = r["props"]
    fl = ", ".join(f"{k} {v}" for k, v in sorted(p["failures"].items(), key=lambda x: -x[1]))
    out.append(f"<h3>Random tests</h3><p>{p['runs']} random worlds and inputs: {p['succeeded']} succeeded "
               f"and were checked against every guarantee; refused: {fl or 'none'}.</p>")
    if p["violations"]:
        out.append("<p class='failc'>Guarantee violated:</p><pre>" + html.escape(
            "\n".join(f"{v.get('eg', '')}   # breaks: {v.get('clause', v.get('error'))}" for v in p["violations"])) + "</pre>")

    m = r["mutation"]
    if m:
        pct = round(100 * m["killed"] / m["total"]) if m["total"] else 100
        out.append(f"<h3>Spec strength <span class='muted'>{m['killed']}/{m['total']} deliberate bugs caught ({pct}%)</span></h3>")
        if m["survived"]:
            out.append("<p>These changes to the body break nothing the spec promises — either the spec is "
                       "missing a guarantee, or the code is unnecessary:</p><ul class='ex'>" +
                       "".join(f"<li>{html.escape(s)}</li>" for s in m["survived"]) + "</ul>")
    else:
        out.append("<h3>Spec strength</h3><p class='muted'>Not measured until every guarantee holds.</p>")

    if r["relied_on"]:
        out.append("<h3>Relies on</h3><p>" + ", ".join(f"<code>{c}</code>" for c in r["relied_on"]) +
                   " — through their specs only, not their code.</p>")

    if why["count"]:
        stale = ("<p class='failc'>Written for build " + why["anchored_to"] +
                 "; the code has changed since. Confirm or update.</p>") if why["stale"] else ""
        out.append(f"<details><summary>Why ({why['count']})</summary>{stale}<ul>" +
                   "".join(f"<li>{html.escape(e)}</li>" for e in why["entries"]) + "</ul></details>")
    out.append("</section>")
    return "\n".join(out)


CSS = """
:root{--bg:#f7f7f5;--card:#fff;--fg:#1d1d1b;--muted:#6b6b66;--line:#e4e4df;--ok:#1f7a4d;--fail:#b3261e;--warn:#a15c00;--code:#f0f0ec}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--bg:#161615;--card:#1f1f1d;--fg:#ececea;--muted:#9a9a94;--line:#33332f;--ok:#5cc28d;--fail:#ff7b72;--warn:#e3a54a;--code:#2a2a27}}
:root[data-theme="dark"]{--bg:#161615;--card:#1f1f1d;--fg:#ececea;--muted:#9a9a94;--line:#33332f;--ok:#5cc28d;--fail:#ff7b72;--warn:#e3a54a;--code:#2a2a27}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:860px;margin:0 auto;padding:32px 16px}h1{font-size:22px;margin:0 0 4px}.sub{color:var(--muted);margin:0 0 24px}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:20px 22px;margin-bottom:20px}
.card header{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.card h2{margin:0;font-size:19px}
.badge{font-size:12px;font-weight:600;padding:2px 9px;border-radius:99px;border:1px solid currentColor}
.badge.ok{color:var(--ok)}.badge.fail{color:var(--fail)}.badge.warn{color:var(--warn)}.hash{margin-left:auto;color:var(--muted);font:12px ui-monospace,monospace}
h3{font-size:13px;text-transform:uppercase;letter-spacing:.04em;color:var(--muted);margin:18px 0 6px}h3 .muted{text-transform:none;letter-spacing:0;font-weight:400}
p{margin:4px 0}.muted{color:var(--muted)}code,pre{font:13px ui-monospace,SFMono-Regular,Menlo,monospace}code{background:var(--code);padding:1px 5px;border-radius:4px}
pre{background:var(--code);padding:8px 10px;border-radius:6px;overflow-x:auto;margin:6px 0}
table{border-collapse:collapse}td{padding:3px 14px 3px 0;vertical-align:top}
ul.g{list-style:none;padding:0;margin:0}ul.g li{display:flex;gap:10px;padding:8px 0;border-top:1px solid var(--line)}ul.g li:first-child{border-top:0}
.ic{font-weight:700;width:16px;flex:none}li.proven .ic{color:var(--ok)}li.refuted .ic{color:var(--fail)}li.conditional .ic,li.unknown .ic{color:var(--warn)}
.how{color:var(--muted);font-size:14px}.src{margin-top:2px;font-size:12px}ul.ex{margin:4px 0;padding-left:18px}
.okc{color:var(--ok)}.failc{color:var(--fail)}details{margin-top:16px;border-top:1px solid var(--line);padding-top:10px}summary{cursor:pointer;font-weight:600}
"""


def page(reports, whys, title):
    cards = "\n".join(card(r, whys[n]) for n, r in reports.items())
    return (f"<!doctype html><html><head><meta charset='utf-8'><meta name='viewport' content='width=device-width,initial-scale=1'>"
            f"<title>Kern proof: {html.escape(title)}</title><style>{CSS}</style></head><body><main>"
            f"<h1>Proof page · {html.escape(title)}</h1><p class='sub'>Generated from verifier results by fixed templates — "
            f"no text here was written by a model.</p>{cards}</main></body></html>")


def terminal(reports, whys):
    out = []
    for n, r in reports.items():
        cls, label = _status(r)
        out.append(f"{n} [{r['hash']}]  {label}")
        for g in r["guarantees"]:
            line = f"  {ICON[g['status']]} {g['kind']} {g['clause']}"
            if g["status"] == "refuted":
                line += f"\n      counterexample: {g['counterexample']['eg']}"
                if not g["counterexample"]["confirmed_by_interpreter"]:
                    line += "   (not reproduced in the interpreter)"
                if g["kind"] == "emit":
                    line += f"\n      audit log got: {g['counterexample']['emitted']}"
            if g["status"] == "unknown" and g.get("reason"):
                line += f"\n      cannot be proven: {g['reason']}"
            if g["status"] == "conditional":
                line += f"   (relies on {', '.join(g['blocked_by'])}, which is not proven)"
                if g.get("via_broken_spec"):
                    line += "\n      refuted only via that broken spec; not reproduced in the interpreter"
            out.append(line)
        ex = r["examples"]
        out.append(f"  examples {sum(e['pass'] for e in ex)}/{len(ex)}   random tests {r['props']['succeeded']}/"
                   f"{r['props']['runs']} reached success, {len(r['props']['violations'])} violations")
        if r["mutation"]:
            m = r["mutation"]
            out.append(f"  spec strength {m['killed']}/{m['total']}" + "".join(f"\n      survived: {s}" for s in m["survived"]))
        w = whys[n]
        if w["count"]:
            out.append(f"  why {w['count']}" + ("  STALE (code changed since written)" if w["stale"] else ""))
        out.append("")
    return "\n".join(out)
