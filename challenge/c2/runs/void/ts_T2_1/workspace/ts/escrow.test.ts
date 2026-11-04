import { test } from "node:test";
import assert from "node:assert/strict";
import { Audit, Ledger, atomic, type Ctx } from "./platform.ts";
import * as escrow from "./escrow.ts";
import { Refused, type EscrowEvent } from "./escrow.ts";

function ctx(balances: Record<string, number>): Ctx<EscrowEvent> {
  return { ledger: new Ledger(new Map(Object.entries(balances))), audit: new Audit(), now: 0 };
}

function run(c: Ctx<EscrowEvent>, fn: () => void): string {
  try {
    atomic(c, fn);
    return "ok";
  } catch (e) {
    if (!(e instanceof Refused)) throw e;
    return "short" in e.reason ? `${e.reason.kind} ${e.reason.short}` : e.reason.kind;
  }
}

function expectRun(balances: Record<string, number>, fn: (c: Ctx<EscrowEvent>) => void,
                   outcome: string, after: Record<string, number> = {}): void {
  const c = ctx(balances);
  assert.equal(run(c, () => fn(c)), outcome);
  for (const [a, b] of Object.entries(after)) assert.equal(c.ledger.balance(a), b);
}

test("transfer", () => {
  expectRun({ A: 500, B: 0 }, (c) => escrow.transfer(c, "A", "B", 100), "ok", { A: 400, B: 100 });
  expectRun({ A: 50, B: 0 }, (c) => escrow.transfer(c, "A", "B", 100), "Insufficient 50");
  expectRun({ A: 500, B: 0 }, (c) => escrow.transfer(c, "A", "A", 100), "SameAccount");
  expectRun({ A: 500, B: 0 }, (c) => escrow.transfer(c, "A", "B", 0), "BadAmount");
});

test("move", () => {
  expectRun({ A: 500, B: 0 }, (c) => escrow.move(c, "A", "B", 0), "ok", { A: 500, B: 0 });
  expectRun({ A: 500, B: 0 }, (c) => escrow.move(c, "A", "B", 100), "ok", { A: 400, B: 100 });
  expectRun({ A: 0 }, (c) => escrow.move(c, "A", "A", 0), "ok", { A: 0 });
});

test("reserve and release", () => {
  expectRun({ A: 500, E: 0 }, (c) => escrow.reserve(c, "A", "E", 100), "ok", { A: 400, E: 100 });
  expectRun({ A: 50, E: 0 }, (c) => escrow.reserve(c, "A", "E", 100), "Insufficient 50");
  expectRun({ A: 400, E: 100 }, (c) => escrow.release(c, "E", "A", 100), "ok", { A: 500, E: 0 });
});

test("capture", () => {
  expectRun({ E: 100, B: 0, H: 0 }, (c) => escrow.capture(c, "E", "B", "H", 50, 5), "ok", { E: 50, B: 50, H: 0 });
  expectRun({ E: 500, B: 0, H: 0 }, (c) => escrow.capture(c, "E", "B", "H", 200, 10), "ok", { E: 300, B: 190, H: 10 });
  expectRun({ E: 500, B: 0, H: 0 }, (c) => escrow.capture(c, "E", "B", "H", 200, 300), "BadFee");
});

test("purchase and cancel", () => {
  expectRun({ A: 500, E: 0, B: 0, H: 0 }, (c) => escrow.purchase(c, "A", "E", "B", "H", 200, 10), "ok",
    { A: 300, E: 0, B: 190, H: 10 });
  expectRun({ A: 400, E: 100 }, (c) => escrow.cancel(c, "E", "A", 100), "ok", { A: 500, E: 0 });
});

test("refund", () => {
  expectRun({ E: 100, A: 0, B: 0, H: 0 }, (c) => escrow.refund(c, "E", "A", "B", "H", 100, 100, 0), "ok",
    { E: 0, A: 100, B: 0, H: 0 });
  expectRun({ E: 300, A: 0, B: 0, H: 0 }, (c) => escrow.refund(c, "E", "A", "B", "H", 300, 100, 10), "ok",
    { E: 0, A: 100, B: 190, H: 10 });
});

test("split and tip", () => {
  expectRun({ E: 100, A: 0, B: 0 }, (c) => escrow.split(c, "E", "A", "B", 100, 30), "ok", { E: 0, A: 30, B: 70 });
  expectRun({ E: 100, A: 0, B: 0 }, (c) => escrow.split(c, "E", "A", "B", 100, 100), "ok", { E: 0, A: 100, B: 0 });
  expectRun({ A: 10, B: 0 }, (c) => escrow.tip(c, "A", "B", 0), "ok", { A: 10, B: 0 });
  expectRun({ A: 10, B: 0 }, (c) => escrow.tip(c, "A", "B", 5), "ok", { A: 5, B: 5 });
});
