transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.owner from = ledger.owner to else SameOwner
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  eg A:500@u1 B:0@u1 | transfer A B 100 -> ok | A:400 B:100
  eg A:500@u1 B:0@u1 | transfer A B 600 -> Insufficient 100
  eg A:100@u1 B:0@u1 | transfer A B 100 -> ok | A:0 B:100
  eg A:500@u1 B:0@u2 | transfer A B 100 -> SameOwner
body
  have = ledger.balance from
  need have >= amount else Insufficient (amount - have)
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (TransferDone from to amount clock.now)

payout from:Account to:Account house:Account amount:Int fee:Int
  need fee >= 0 else BadAmount
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount - fee
  eg A:500@u1 B:0@u1 H:0@u1 | payout A B H 100 5 -> ok | A:395 B:100 H:5
body
  transfer from to amount
  transfer from house fee
