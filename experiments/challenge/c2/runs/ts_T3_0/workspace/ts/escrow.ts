import type { Account, Ctx } from "./platform.ts";

export type EscrowEvent =
  | { kind: "Moved"; from: Account; to: Account; amount: number; at: number }
  | { kind: "Reserved"; buyer: Account; amount: number; at: number }
  | { kind: "Released"; buyer: Account; amount: number; at: number }
  | { kind: "Captured"; seller: Account; amount: number; at: number }
  | { kind: "Cancelled"; buyer: Account; at: number }
  | { kind: "Tipped"; seller: Account; amount: number; at: number };

export type Refusal =
  | { kind: "BadAmount" }
  | { kind: "BadFee" }
  | { kind: "BadPart" }
  | { kind: "SameAccount" }
  | { kind: "Insufficient"; short: number }
  | { kind: "Overdrawn" };

export class Refused extends Error {
  readonly reason: Refusal;
  constructor(reason: Refusal) {
    super(reason.kind);
    this.reason = reason;
  }
}

type C = Ctx<EscrowEvent>;

function refuse(reason: Refusal): never {
  throw new Refused(reason);
}

function needFunds(c: C, a: Account, amount: number): void {
  const bal = c.ledger.balance(a);
  if (bal < amount) refuse({ kind: "Insufficient", short: amount - bal });
}

export function transfer(c: C, from: Account, to: Account, amount: number): void {
  if (amount <= 0) refuse({ kind: "BadAmount" });
  if (from === to) refuse({ kind: "SameAccount" });
  needFunds(c, from, amount);
  c.ledger.debit(from, amount);
  c.ledger.credit(to, amount);
  c.audit.emit({ kind: "Moved", from, to, amount, at: c.now });
}

/** Like transfer, but moving 0 is allowed and does nothing (even when from = to). */
function moveLoose(c: C, from: Account, to: Account, amount: number): void {
  if (amount < 0) refuse({ kind: "BadAmount" });
  if (amount > 0) transfer(c, from, to, amount);
}

/** Like transfer, but moving 0 between distinct accounts is allowed and does nothing. */
export function move(c: C, from: Account, to: Account, amount: number): void {
  if (amount < 0) refuse({ kind: "BadAmount" });
  if (from === to) refuse({ kind: "SameAccount" });
  moveLoose(c, from, to, amount);
}

export function reserve(c: C, buyer: Account, escrow: Account, amount: number): void {
  transfer(c, buyer, escrow, amount);
  c.audit.emit({ kind: "Reserved", buyer, amount, at: c.now });
}

export function release(c: C, escrow: Account, buyer: Account, amount: number): void {
  transfer(c, escrow, buyer, amount);
  c.audit.emit({ kind: "Released", buyer, amount, at: c.now });
}

/** Pays the seller from escrow. From 100 up, the house takes `fee`. */
export function capture(c: C, escrow: Account, seller: Account, house: Account, amount: number, fee: number): void {
  if (amount <= 0) refuse({ kind: "BadAmount" });
  if (fee < 0 || fee > amount) refuse({ kind: "BadFee" });
  if (escrow === seller || escrow === house || seller === house) refuse({ kind: "SameAccount" });
  needFunds(c, escrow, amount);
  if (amount < 100) {
    transfer(c, escrow, seller, amount);
  } else {
    move(c, escrow, seller, amount - fee);
    move(c, escrow, house, fee);
  }
  c.audit.emit({ kind: "Captured", seller, amount, at: c.now });
}

export function purchase(
  c: C, buyer: Account, escrow: Account, seller: Account, house: Account, amount: number, fee: number,
): void {
  if (amount <= 0) refuse({ kind: "BadAmount" });
  if (fee < 0 || fee > amount) refuse({ kind: "BadFee" });
  if (buyer === escrow || escrow === seller || escrow === house || seller === house) {
    refuse({ kind: "SameAccount" });
  }
  needFunds(c, buyer, amount);
  if (c.ledger.balance(escrow) < 0) refuse({ kind: "Overdrawn" });
  reserve(c, buyer, escrow, amount);
  capture(c, escrow, seller, house, amount, fee);
}

export function cancel(c: C, escrow: Account, buyer: Account, amount: number): void {
  release(c, escrow, buyer, amount);
  c.audit.emit({ kind: "Cancelled", buyer, at: c.now });
}

/** Gives `part` back to the buyer; the rest is captured for the seller. */
export function refund(
  c: C, escrow: Account, buyer: Account, seller: Account, house: Account, amount: number, part: number, fee: number,
): void {
  if (part <= 0 || part > amount) refuse({ kind: "BadPart" });
  if (part !== amount && (fee < 0 || fee > amount - part)) refuse({ kind: "BadFee" });
  if (escrow === buyer || escrow === seller || escrow === house || seller === house) {
    refuse({ kind: "SameAccount" });
  }
  needFunds(c, escrow, amount);
  if (part === amount) {
    release(c, escrow, buyer, amount);
  } else {
    move(c, escrow, buyer, part);
    capture(c, escrow, seller, house, amount - part, fee);
  }
}

export function split(c: C, escrow: Account, a: Account, b: Account, amount: number, part: number): void {
  if (part < 0 || part > amount) refuse({ kind: "BadPart" });
  if (escrow === a || escrow === b || a === b) refuse({ kind: "SameAccount" });
  needFunds(c, escrow, amount);
  move(c, escrow, a, part);
  move(c, escrow, b, amount - part);
}

export function tip(c: C, buyer: Account, seller: Account, amount: number): void {
  moveLoose(c, buyer, seller, amount);
  c.audit.emit({ kind: "Tipped", seller, amount, at: c.now });
}
