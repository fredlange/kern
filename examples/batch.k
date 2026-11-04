state hold Int
state expiry Int
state sub_amount Int
state sub_due Int

invariant o:Int hold.get o >= 0
invariant o:Int sub_amount.get o >= 0
invariant a:Account ledger.balance a >= 0

transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit Moved from to amount clock.now
  eg A:500@u1 B:0@u2 | transfer A B 100 -> ok | A:400 B:100
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (Moved from to amount clock.now)

sweep_expired escrow:Account pool:Account lo:Int hi:Int
  need escrow != pool else SameAccount
  keep ledger.total
  ensure ledger.balance' escrow + ledger.balance' pool = ledger.balance escrow + ledger.balance pool
  ensure hold.get' j = 0 when expiry.get j < clock.now for j from lo to hi
  ensure hold.get' j = hold.get j when expiry.get j >= clock.now for j from lo to hi
  eg E:100@u0 P:0@u9 hold.1:30 hold.2:50 expiry.1:5 expiry.2:99 now:10 | sweep_expired E P 1 3 -> ok | E:70 P:30 hold.1:0 hold.2:50
  eg E:10@u0 P:0@u9 hold.1:30 expiry.1:5 now:10 | sweep_expired E P 1 2 -> Insufficient 20
body
  for k from lo to hi
    invariant ledger.total' = ledger.total
    invariant ledger.balance' escrow + ledger.balance' pool = ledger.balance escrow + ledger.balance pool
    invariant hold.get' j = 0 when expiry.get j < clock.now for j from lo to k
    invariant hold.get' j = hold.get j when expiry.get j >= clock.now for j from lo to k
    if expiry.get k < clock.now and hold.get k > 0
      transfer escrow pool (hold.get k)
      hold.set k 0

advance_subs lo:Int hi:Int
  ok otherwise
  keep ledger.total
  ensure sub_due.get' j = sub_due.get j + 30 when sub_amount.get j > 0 and sub_due.get j <= clock.now for j from lo to hi
  ensure sub_due.get' j = sub_due.get j when sub_amount.get j = 0 or sub_due.get j > clock.now for j from lo to hi
  eg sub_amount.1:40 sub_due.1:5 sub_due.2:5 now:10 | advance_subs 1 3 -> ok | sub_due.1:35 sub_due.2:5
body
  for k from lo to hi
    invariant ledger.total' = ledger.total
    invariant sub_due.get' j = sub_due.get j + 30 when sub_amount.get j > 0 and sub_due.get j <= clock.now for j from lo to k
    invariant sub_due.get' j = sub_due.get j when sub_amount.get j = 0 or sub_due.get j > clock.now for j from lo to k
    if sub_amount.get k > 0 and sub_due.get k <= clock.now
      sub_due.set k (sub_due.get k + 30)

installments buyer:Account seller:Account n:Int
  need n >= 0 else BadCount
  need buyer != seller else SameAccount
  need ledger.balance buyer >= 10 * n else Insufficient (10 * n - ledger.balance buyer)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - 10 * n
  ensure ledger.balance' seller = ledger.balance seller + 10 * n
  eg A:100@u1 B:0@u2 | installments A B 3 -> ok | A:70 B:30
  eg A:20@u1 B:0@u2 | installments A B 3 -> Insufficient 10
body
  for k from 0 to n
    invariant ledger.total' = ledger.total
    invariant ledger.balance' buyer = ledger.balance buyer - 10 * k
    invariant ledger.balance' seller = ledger.balance seller + 10 * k
    transfer buyer seller 10

sweep_elsewhere escrow:Account pool:Account lo:Int hi:Int other:Int
  need escrow != pool else SameAccount
  need other < lo or other >= hi else InRange
  keep ledger.total
  ensure hold.get' other = hold.get other
  eg E:100@u0 P:0@u9 hold.1:30 hold.7:5 expiry.1:5 expiry.7:5 now:10 | sweep_elsewhere E P 1 3 7 -> ok | hold.7:5 hold.1:0
body
  sweep_expired escrow pool lo hi
