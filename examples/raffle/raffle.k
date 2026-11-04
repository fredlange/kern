handler rand.below n:Int -> Int
  need n > 0 else BadBound
  ensure result >= 0 and result < n

state picked Int

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

draw_winner pot:Account a:Account b:Account c:Account amount:Int
  need distinct pot a b c else SameAccount
  need amount > 0 else BadAmount
  need ledger.balance pot >= amount else Insufficient (amount - ledger.balance pot)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' pot = ledger.balance pot - amount
  emit Moved pot _ amount clock.now
  emit Won _ amount clock.now
  eg P:100@u0 A:0@u1 B:0@u2 C:0@u3 | draw_winner P A B C 30 -> ok | P:70
  eg P:10@u0 A:0@u1 B:0@u2 C:0@u3 | draw_winner P A B C 30 -> Insufficient 20
body
  r = rand.below 3
  if r = 0
    transfer pot a amount
    audit.emit (Won a amount clock.now)
  else
    if r = 1
      transfer pot b amount
      audit.emit (Won b amount clock.now)
    else
      transfer pot c amount
      audit.emit (Won c amount clock.now)

pick_order lo:Int hi:Int
  need lo < hi else Empty
  ok otherwise
  keep ledger.total
  ensure picked.get' 0 >= lo and picked.get' 0 < hi
  emit Picked _ clock.now
  eg | pick_order 5 9 -> ok
  eg | pick_order 5 5 -> Empty
body
  k = lo + rand.below (hi - lo)
  picked.set 0 k
  audit.emit (Picked k clock.now)
