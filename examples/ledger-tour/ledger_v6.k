transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.owner from = ledger.owner to else SameOwner
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit TransferDone from to amount clock.now
  eg A:500@u1 B:0@u1 | transfer A B 100 -> ok | A:400 B:100
  eg A:500@u1 B:0@u1 | transfer A B 600 -> Insufficient 100
  eg A:100@u1 B:0@u1 | transfer A B 100 -> ok | A:0 B:100
  eg A:500@u1 B:0@u2 | transfer A B 100 -> SameOwner
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (TransferDone from to amount clock.now)

payout from:Account to:Account house:Account amount:Int fee:Int
  need amount > 0 and fee >= 0 else BadAmount
  need from != to and from != house else SameAccount
  need ledger.owner from = ledger.owner to and ledger.owner from = ledger.owner house else SameOwner
  need ledger.balance from >= amount + fee else Insufficient (amount + fee - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount - fee
  emit TransferDone from to amount clock.now
  emit TransferDone from house fee clock.now
  eg A:500@u1 B:0@u1 H:0@u1 | payout A B H 100 5 -> ok | A:395 B:100 H:5
body
  transfer from to amount
  transfer from house fee
