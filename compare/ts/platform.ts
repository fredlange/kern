// Not counted: the equivalent of Kern's handlers (ledger, clock, audit) and atomic run().
export type Account = string;

export class Ledger {
  balances: Map<Account, number>;
  constructor(balances: Map<Account, number>) { this.balances = balances; }
  balance(a: Account): number {
    const b = this.balances.get(a);
    if (b === undefined) throw new Error(`unknown account ${a}`);
    return b;
  }
  debit(a: Account, n: number): void { this.balances.set(a, this.balance(a) - n); }
  credit(a: Account, n: number): void { this.balances.set(a, this.balance(a) + n); }
  total(): number { let t = 0; for (const b of this.balances.values()) t += b; return t; }
}

export class Audit<E> {
  events: E[] = [];
  emit(e: E): void { this.events.push(e); }
}

export interface Ctx<E> { ledger: Ledger; audit: Audit<E>; now: number }

/** Run fn atomically: on any throw, balances and audit are restored. */
export function atomic<E, T>(c: Ctx<E>, fn: () => T): T {
  const bal = new Map(c.ledger.balances), n = c.audit.events.length;
  try {
    return fn();
  } catch (e) {
    c.ledger.balances = bal;
    c.audit.events.length = n;
    throw e;
  }
}
