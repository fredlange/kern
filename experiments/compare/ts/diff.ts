// Not counted: differential harness. Reads JSON-lines cases on stdin, prints results.
import { readFileSync } from "node:fs";
import { Audit, Ledger, atomic, type Ctx } from "./platform.ts";
import * as escrow from "./escrow.ts";
import { Refused, type EscrowEvent } from "./escrow.ts";

const ORDER: Record<string, string[]> = {
  Moved: ["from", "to", "amount", "at"], Reserved: ["buyer", "amount", "at"], Released: ["buyer", "amount", "at"],
  Captured: ["seller", "amount", "at"], Cancelled: ["buyer", "at"], Tipped: ["seller", "amount", "at"],
};
for (const line of readFileSync(0, "utf8").split("\n")) {
  if (!line) continue;
  const k = JSON.parse(line);
  const c: Ctx<EscrowEvent> = { ledger: new Ledger(new Map(Object.entries(k.world))), audit: new Audit(), now: k.now };
  let outcome: unknown = ["ok"];
  try {
    atomic(c, () => (escrow as any)[k.def](c, ...k.args));
  } catch (e) {
    if (!(e instanceof Refused)) throw e;
    outcome = "short" in e.reason ? [e.reason.kind, [e.reason.short]] : [e.reason.kind, []];
  }
  const events = c.audit.events.map((ev: any) => [ev.kind, ...ORDER[ev.kind].map((f) => ev[f])]);
  console.log(JSON.stringify({ outcome, bal: Object.fromEntries(c.ledger.balances), events }));
}
