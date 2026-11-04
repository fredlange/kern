import chance
import money.ledger

draw pot:Account a:Account b:Account c:Account amount:Int
  need distinct pot a b c else SameAccount
  need amount > 0 else BadAmount
  need ledger.balance pot >= amount else Insufficient (amount - ledger.balance pot)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' pot = ledger.balance pot - amount
  emit Moved pot _ amount clock.now
  emit Won _ amount clock.now
  eg P:100@u0 A:0@u1 B:0@u2 C:0@u3 | draw P A B C 30 -> ok | P:70
  eg P:10@u0 A:0@u1 B:0@u2 C:0@u3 | draw P A B C 30 -> Insufficient 20
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
