channel net

state pending Account
state req Account
state limit Account
state spent Account

invariant a:Account pending.get a = 0 or pending.get a = 1
invariant a:Account ledger.balance a >= 0
invariant a:Account limit.get a >= 0

transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit Moved from to amount clock.now
  eg A:500@u1 K:0@u9 | transfer A K 100 -> ok | A:400 K:100
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (Moved from to amount clock.now)

request_limit a:Account
  need pending.get a = 0 else AlreadyPending
  ok otherwise
  keep ledger.total
  ensure pending.get' a = 1
  ensure req.get' a = req.get a + 1
  emit net Fetch a (req.get a + 1)
  eg A:0@u1 | request_limit A -> ok | pending.A:1 req.A:1
  eg A:0@u1 pending.A:1 | request_limit A -> AlreadyPending
body
  req.set a (req.get a + 1)
  pending.set a 1
  net.emit (Fetch a (req.get a))

got_limit a:Account n:Int value:Int
  need pending.get a = 1 else Stale
  need req.get a = n else Stale
  need value >= 0 else BadValue
  ok otherwise
  keep ledger.total
  ensure limit.get' a = value
  ensure pending.get' a = 0
  emit LimitSet a value clock.now
  eg A:0@u1 pending.A:1 req.A:3 | got_limit A 3 250 -> ok | limit.A:250 pending.A:0
  eg A:0@u1 pending.A:1 req.A:3 | got_limit A 2 250 -> Stale
  eg A:0@u1 req.A:3 | got_limit A 3 250 -> Stale
body
  limit.set a value
  pending.set a 0
  audit.emit (LimitSet a value clock.now)

limit_failed a:Account n:Int
  need pending.get a = 1 else Stale
  need req.get a = n else Stale
  ok otherwise
  keep ledger.total
  ensure pending.get' a = 0
  ensure limit.get' a = limit.get a
  emit LimitFailed a clock.now
  eg A:0@u1 pending.A:1 req.A:3 limit.A:40 | limit_failed A 3 -> ok | pending.A:0 limit.A:40
body
  pending.set a 0
  audit.emit (LimitFailed a clock.now)

withdraw a:Account bank:Account amount:Int
  need spent.get a + amount <= limit.get a else OverLimit (spent.get a + amount - limit.get a)
  need as transfer a bank amount
  ok otherwise
  keep ledger.total
  ensure spent.get' a = spent.get a + amount
  ensure ledger.balance' a = ledger.balance a - amount
  emit as transfer a bank amount
  eg A:500@u1 K:0@u9 limit.A:300 | withdraw A K 200 -> ok | A:300 K:200 spent.A:200
  eg A:500@u1 K:0@u9 limit.A:300 spent.A:200 | withdraw A K 200 -> OverLimit 100
body
  transfer a bank amount
  spent.set a (spent.get a + amount)

reset_day a:Account
  ok otherwise
  keep ledger.total
  ensure spent.get' a = 0
  eg A:0@u1 spent.A:70 | reset_day A -> ok | spent.A:0
body
  spent.set a 0
