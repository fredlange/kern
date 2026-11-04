"""Proof page (HTML) and terminal summary, rendered from verifier results with fixed
templates. Nothing here is written by a language model, so the page cannot say more
than the checks established."""
from __future__ import annotations

import html

from .infer import fails as inferred_fails
from .infer import fmt_writes
from .syntax import code, flat, negate
from .views import EFFECTS, en, en_emits, en_ensure, en_refusal, guarded_needs, lets_of, unguaranteed_writes

ICON = {"proven": "✔", "refuted": "✗", "conditional": "!", "unknown": "?", "unconfirmed": "?",
        "tested": "◐", "vacuous": "∅", "assumed": "⊙"}


def _status(r):
    gs = r["guarantees"]
    if any(g["status"] == "vacuous" for g in gs):
        return "fail", "Never succeeds"
    if any(g["status"] == "refuted" for g in gs) or r["props"]["violations"] or not all(e["pass"] for e in r["examples"]):
        return "fail", "Broken guarantees"
    if r.get("regression"):
        if all(x.startswith("spec changed") for x in r["regression"]):
            return "warn", "Spec changed since baseline"
        return "warn", "Weaker than baseline"
    if not gs:
        return "warn", "No guarantees stated"
    if any(g["status"] == "conditional" for g in gs):
        return "warn", "Depends on unproven code"
    if any(g["status"] == "unconfirmed" for g in gs):
        return "warn", "Not proven: counterexample not reproduced"
    if any(g["status"] == "tested" for g in gs):
        return "warn", "Tested only, not proven"
    if any(g["status"] not in ("proven", "assumed") for g in gs):
        return "warn", "Not proven: prover gave no answer"
    assumed = sorted({a for g in gs for a in g.get("assuming", [])})
    if assumed:
        return "ok", "Proven, assuming " + ", ".join(assumed)
    return "ok", "All guarantees proven"


def _guarantee_text(g, d):
    kind, e = g["kind"], g["expr"]
    if kind == "emit":
        return en_emits(e)
    if kind == "ok":
        return "succeeds whenever the needs above hold: it refuses for no other reason"
    if kind == "loop":
        part, ast = e
        when = "holds when the loop starts" if part == "entry" else "is kept by every iteration"
        what = html.escape(ast[1]) if ast[0] == "module" else en_ensure(ast)
        return (f"loop invariant ({when}; at the start of each iteration, primed reads are the current "
                f"state and unprimed reads the state before the call): {what}")
    if kind == "invariant":
        var, t, cond = e
        return f"keeps the module invariant for every <code>{var}</code> ({t}): {en(cond)}"
    return f"{en(e)} is unchanged" if kind == "keep" else en_ensure(e)


def _how(g):
    if g["status"] == "proven":
        return "Proven for all inputs (Z3)."
    if g["status"] == "assumed":
        who = ", ".join(f"<code>{html.escape(a)}</code>" for a in g["assuming"])
        return (f"Proven for all inputs, <em>assuming</em> the declared spec of {who}: an observation the "
                "host provides. The host checks that spec on every call; the prover can't.")
    if g["status"] == "conditional":
        who = ", ".join(f"<code>{c}</code>" for c in g["blocked_by"])
        if g.get("via_broken_spec"):
            return (f"Unknown: it fails according to the spec of {who}, but that spec is itself broken, "
                    f"and the interpreter did not reproduce the failure.")
        return f"Proven only if the guarantees of {who} hold — and they are not proven."
    if g["status"] == "vacuous":
        return f"Holds only vacuously: {html.escape(g['reason'])}."
    if g["status"] == "tested":
        return (f"Not proven. Held in all {g['tested']} random runs that succeeded"
                + (f" ({html.escape(g['reason'])})." if g.get("reason") else "."))
    if g["status"] in ("refuted", "unconfirmed"):
        cx = g["counterexample"]
        conf = (" Replayed in the interpreter: confirmed." if cx["confirmed_by_interpreter"]
                else " Not reproduced in the interpreter, so this is not evidence of a bug: a callee's "
                     "spec may be too weak to prove the guarantee.")
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

    eff = ", ".join(EFFECTS.get(e, f"state <code>{e.rsplit('.', 1)[0]}</code> (read {'and write' if e.endswith('write') else 'only'})")
                    for e in sorted(r["uses"])) or "nothing"
    out.append(f'<h3>Can touch</h3><p>{eff}. <span class="muted">Inferred from the body; nothing else is reachable.</span></p>')
    if r["writes"]:
        what = "Changes" if any("." in w for w in r["writes"]) else "Changes balances of"
        out.append(f"<p>{what}: {html.escape(fmt_writes(r['writes'], d))}. "
                   f"<span class='muted'>Inferred; every other {'balance and state entry' if what == 'Changes' else 'balance'} is unchanged.</span></p>")
        silent = unguaranteed_writes(r)
        if silent:
            out.append(f"<p class='warnc'>No guarantee says what the new {'value' if all('.' in x for x in silent) else 'balance'} of " +
                       ", ".join(f"<code>{p}</code>" for p in silent) + " is.</p>")
    from .views import loop_event_note
    for ch in loop_event_note(r):
        out.append(f"<p class='warnc'>Events on <code>{html.escape(ch)}</code> emitted inside a loop are not "
                   "described by any spec: a caller can't rely on them (there is no per-iteration emit form yet).</p>")

    rows = []
    origin = {id(n): d.ref_origin.get(("need", i)) for i, n in enumerate(d.needs)}
    for n, guards in [(n, []) for n in d.needs] + list(guarded_needs(d.body)):
        cond, _ = en_refusal(n, lets)
        if origin.get(id(n)):                       # written as `need as X …`: X's spec, these arguments
            cond += f" <span class='muted'>(as {html.escape(origin[id(n)])})</span>"
        for kind, g in reversed(guards):           # a need inside `if G` only applies when G holds
            g_text = en(g, lets) if kind == "if" else (en(negate(g), lets) if negate(g) else f"not ({en(g, lets)})")
            cond = f"{g_text} <span class='muted'>and</span> {cond}"
        detail = f" <span class='muted'>with</span> {', '.join(en(a, lets) for a in n[3])}" if n[3] else ""
        rows.append((cond, f"<code>{html.escape(n[2])}</code>{detail}"))
    ok = next((g for g in r["guarantees"] if g["kind"] == "ok"), None)
    complete = ok is not None and ok["status"] in ("proven", "conditional")
    for cond, ctor, payload, origin in ([] if complete else r["lifted"]):
        if origin:                                  # own body needs are already listed above
            text, _ = en_refusal(("need", cond, ctor, payload or []), {})
            rows.append((text, f"<code>{html.escape(ctor)}</code> <span class='muted'>(from {origin})</span>"))
    stated = {(origin, ctor) for _, ctor, _, origin in r["lifted"]}
    own = {n[2] for n in flat(d.needs + d.body) if n[0] == "need"}
    for c in ([] if complete else r["callees"]):
        inherited = [k for k in inferred_fails(r["defs"], r["defs"][c])
                     if k not in own and not any(k == ctor for _, ctor in stated)]
        if inherited:
            rows.append((f"<code>{c}</code> refuses",
                         ", ".join(f"<code>{k}</code>" for k in inherited) + " <span class='muted'>(passed through)</span>"))
    if rows:
        out.append("<h3>Refuses when</h3><table>" + "".join(
            f"<tr><td>{cond}</td><td class='fail-name'>→ {f}</td></tr>" for cond, f in rows)
            + "</table><p class='muted'>A refusal undoes everything the call did."
            + (" It refuses for no other reason (see <code>ok otherwise</code> below)." if complete else "")
            + "</p>")

    out.append("<h3>Guarantees</h3><ul class='g'>")
    from .run import emit_channels
    if len(emit_channels(d)) > 1:
        out.append("<p class='muted'>Each channel's events are guaranteed in order; the order <em>across</em> "
                   "channels is not (the host delivers audit first, then channels in declaration order).</p>")
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
        pct = round(100 * m["guarantee_killed"] / m["total"]) if m["total"] else 100
        out.append(f"<h3>Spec strength <span class='muted'>guarantees catch {m['guarantee_killed']}/{m['total']} "
                   f"deliberate bugs ({pct}%)</span></h3>"
                   f"<p>Caught by the prover: {m['prover_killed']}; by random tests checking the guarantees: "
                   f"{m['props_killed']}. The examples alone catch {m['example_killed']}/{m['total']}; "
                   f"everything together {m['killed']}/{m['total']}.</p>")
        if m["guarantee_missed"]:
            out.append("<p>No guarantee rules out these changes to the body. Either a guarantee is missing, "
                       "or the code is unnecessary:</p><ul class='ex'>" +
                       "".join(f"<li>{html.escape(s)}" + (" <span class='muted'>(an example catches it)</span>"
                               if ex else " <span class='failc'>(nothing catches it)</span>") + "</li>"
                               for s, ex in m["guarantee_missed"]) + "</ul>")
        if m["errors"]:
            out.append("<p class='failc'>The checker crashed on these mutants (not counted):</p><ul class='ex'>" +
                       "".join(f"<li>{html.escape(e)}</li>" for e in m["errors"]) + "</ul>")
    else:
        out.append("<h3>Spec strength</h3><p class='muted'>Not measured until every guarantee holds.</p>")

    if r.get("regression"):
        out.append("<h3>Changed since the baseline</h3><p class='muted'>What callers could rely on at the "
                   "baseline and can't now. A change may be intended; it must not be invisible.</p><ul class='ex'>" +
                   "".join(f"<li class='failc'>{html.escape(x)}</li>" for x in r["regression"]) + "</ul>")

    if r["relied_on"]:
        out.append("<h3>Relies on</h3><p>" + ", ".join(f"<code>{c}</code>" for c in r["relied_on"]) +
                   " — through their specs only, not their code.</p>")

    if why["count"]:
        stale = ("<p class='failc'>Written for " + why["anchored_to"] + "; the definition or its inferred "
                 "interface has changed since (now " + why["current"] + "). Confirm or update.</p>") if why["stale"] else ""
        if why.get("obligations"):
            stale += ("<p class='warnc'>These read like obligations. Anything that must be obeyed belongs "
                      "in a need, keep, ensure or emit, not only in prose:</p><ul>" +
                      "".join(f"<li>{html.escape(e)}</li>" for e in why["obligations"]) + "</ul>")
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
.okc{color:var(--ok)}.failc{color:var(--fail)}.warnc{color:var(--warn)}details{margin-top:16px;border-top:1px solid var(--line);padding-top:10px}summary{cursor:pointer;font-weight:600}
"""


def _orphan_html(orphans):
    if not orphans:
        return ""
    return ("<section class='card'><h2>Orphaned why entries</h2><p class='failc'>These explain definitions "
            "that no longer exist (renamed or deleted?):</p><ul>" +
            "".join(f"<li><code>{html.escape(n)}</code>: {html.escape(e)}</li>" for n, es in orphans.items() for e in es)
            + "</ul></section>")


def host_html(host):
    """What the host realises and assumes. Fixed templates over the host's own tables; every line
    is marked *assumed*: none of it is proven, it is what the proofs above rely on the host for.
    `host` = {"effects": {effect: how}, "endpoints": [(ctor, method, url, reply, fail)],
              "assumptions": [text]}."""
    if not host:
        return ""
    rows = "".join(f"<tr><td><code>{html.escape(e)}</code></td><td>{html.escape(how)}</td></tr>"
                   for e, how in host["effects"].items())
    eps = "".join(f"<li><code>{html.escape(c)}</code> → {html.escape(m)} <code>{html.escape(u)}</code>; reply "
                  f"<code>{html.escape(r)}</code>, failure <code>{html.escape(f)}</code></li>"
                  for c, m, u, r, f in host["endpoints"])
    asm = "".join(f"<li><span class='badge warn'>assumed</span> {html.escape(a)}</li>" for a in host["assumptions"])
    return ("<section class='card'><header><h2>Host</h2><span class='badge warn'>assumed, not proven</span></header>"
            "<p class='muted'>The proofs above are about the Kern code. This section lists what the host does "
            "with each effect the module can name, and what the proofs assume about it. None of it is "
            "checked by the prover.</p>"
            f"<h3>Effects and how this host realises them</h3><table>{rows}</table>"
            + (f"<h3>Network endpoints (the only ones the module can reach)</h3><ul>{eps}</ul>" if eps else "")
            + f"<h3>Assumptions</h3><ul>{asm}</ul></section>")


def page(reports, whys, title, orphans=None, host=None):
    lines = assumption_lines(reports)
    top = ("<section class='card'><h2>Assumptions</h2><p class='muted'>Observations this module rests on. "
           "Guarantees marked ⊙ are proven <em>assuming</em> these specs; the prover can't check them, the "
           "host checks them on every call.</p><ul>" +
           "".join(f"<li><code>{html.escape(h)}</code>: <code>{html.escape(sp)}</code> "
                   f"<span class='{'failc' if sat == 'unsatisfiable' else 'muted'}'>(spec {html.escape(sat)})</span></li>"
                   for _, h, sp, sat in lines) + "</ul></section>") if lines else ""
    cards = top + "\n".join(card(r, whys[n]) for n, r in reports.items()) + _orphan_html(orphans) + host_html(host)
    return (f"<!doctype html><html><head><meta charset='utf-8'><meta name='viewport' content='width=device-width,initial-scale=1'>"
            f"<title>Kern proof: {html.escape(title)}</title><style>{CSS}</style></head><body><main>"
            f"<h1>Proof page · {html.escape(title)}</h1><p class='sub'>Generated from verifier results by fixed templates — "
            f"no text here was written by a model.</p>{cards}</main></body></html>")


def assumption_lines(reports):
    """The assumed handlers the module rests on, with their specs and whether they can be met."""
    r = next(iter(reports.values()), None)
    if r is None:
        return []
    out = []
    for name, h in getattr(r["defs"], "handlers", {}).items():
        spec = "; ".join([f"need {code(n[1])}" for n in h.needs] + [f"ensure {code(c)}" for c in h.ensures])
        out.append((name, h.src_lines["header"][0], spec, r.get("handler_sat", {}).get(name, "not checked")))
    return out


def terminal(reports, whys, orphans=None):
    out = []
    for name, header, spec, sat in assumption_lines(reports):
        out.append(f"ASSUMES {header[len('handler '):]}: {spec}   (spec {sat}; the host provides it and checks it on every call)")
    if out:
        out.append("")
    for n, r in reports.items():
        cls, label = _status(r)
        out.append(f"{n} [{r['hash']}]  {label}")
        for g in r["guarantees"]:
            line = f"  {ICON[g['status']]} {g['kind']} {g['clause']}"
            if g["status"] in ("refuted", "unconfirmed"):
                line += f"\n      counterexample: {g['counterexample']['eg']}"
                if not g["counterexample"]["confirmed_by_interpreter"]:
                    line += "   (not reproduced in the interpreter)"
                if g["kind"] == "emit":
                    line += f"\n      audit log got: {g['counterexample']['emitted']}"
            if g["status"] == "unknown" and g.get("reason"):
                line += f"\n      cannot be proven: {g['reason']}"
            if g["status"] == "vacuous":
                line += f"\n      holds only vacuously: {g['reason']}"
            if g["status"] == "tested":
                line += f"   (not proven; held in {g['tested']} random successes)"
            if g["status"] == "assumed":
                line += f"   (assuming {', '.join(g['assuming'])})"
            if g["status"] == "conditional":
                line += f"   (relies on {', '.join(g['blocked_by'])}, which is not proven)"
                if g.get("via_broken_spec"):
                    line += "\n      refuted only via that broken spec; not reproduced in the interpreter"
            out.append(line)
        if r["writes"]:
            silent = unguaranteed_writes(r)
            out.append(f"  writes {fmt_writes(r['writes'], r['def'])}" +
                       (f"   (no guarantee about the new {'value' if all('.' in x for x in silent) else 'balance'} of "
                        f"{', '.join(silent)})" if silent else ""))
        from .views import loop_event_note
        for ch in loop_event_note(r):
            out.append(f"  {ch}: events emitted inside a loop are not described by any spec (no per-iteration emit form yet)")
        ex = r["examples"]
        out.append(f"  examples {sum(e['pass'] for e in ex)}/{len(ex)}   random tests {r['props']['succeeded']}/"
                   f"{r['props']['runs']} reached success, {len(r['props']['violations'])} violations")
        if r.get("mutation_note"):
            out.append(f"  spec strength: {r['mutation_note']}")
        if r["mutation"]:
            m = r["mutation"]
            out.append(f"  spec strength: guarantees {m['guarantee_killed']}/{m['total']} "
                       f"(prover {m['prover_killed']}, random tests {m['props_killed']}); "
                       f"examples {m['example_killed']}/{m['total']}; together {m['killed']}/{m['total']}")
            for s, ex in m["guarantee_missed"]:
                out.append(f"      {'missed' if ex else 'survived'}: {s}" + ("   (an example catches it)" if ex else ""))
            for e in m["errors"]:
                out.append(f"      checker error: {e}")
        for x in r.get("regression", []):
            label = "SPEC CHANGED SINCE BASELINE" if x.startswith("spec changed") else "WEAKER THAN BASELINE"
            out.append(f"  {label}: {x[len('spec changed: '):] if x.startswith('spec changed') else x}")
        w = whys[n]
        if w["count"]:
            out.append(f"  why {w['count']}" + ("  STALE (definition or its interface changed since written)"
                                                 if w["stale"] else ""))
            for e in w.get("obligations", []):
                out.append(f"      why reads like an obligation (move it to the spec?): {e}")
        out.append("")
    for n, es in (orphans or {}).items():
        out.append(f"ORPHANED why: {len(es)} entr{'y' if len(es) == 1 else 'ies'} for `{n}`, which does not exist")
    return "\n".join(out)
