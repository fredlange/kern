transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.owner from = ledger.owner to else SameOwner
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit TransferDone from to amount _
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
  emit TransferDone from to amount _
  emit TransferDone from house fee _
  eg A:500@u1 B:0@u1 H:0@u1 | payout A B H 100 5 -> ok | A:395 B:100 H:5
body
  transfer from to amount
  transfer from house fee

settle from:Account to:Account house:Account amount:Int fee:Int
  need fee <= amount else FeeTooLarge
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount - fee
  emit TransferDone from to amount _
  emit TransferDone from house fee _
  emit Settled from to _
  eg A:500@u1 B:0@u1 H:0@u1 | settle A B H 100 5 -> ok | A:395 B:100 H:5
  eg A:500@u1 B:0@u1 H:0@u1 | settle A B H 5 100 -> FeeTooLarge
body
  payout from to house amount fee
  audit.emit (Settled from to clock.now)

deposit to:Account amount:Int
  need amount > 0 else BadAmount
  ensure ledger.balance' to = ledger.balance to + amount
  ensure ledger.total' = ledger.total + amount
  emit Deposited to amount _
  eg A:0@u1 | deposit A 50 -> ok | A:50
body
  ledger.credit to amount
  audit.emit (Deposited to amount clock.now)

withdraw from:Account amount:Int
  need amount > 0 else BadAmount
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.total' = ledger.total - amount
  eg A:80@u1 | withdraw A 50 -> ok | A:30
  eg A:80@u1 | withdraw A 90 -> Insufficient 10
body
  have = ledger.balance from
  need have >= amount else Insufficient (amount - have)
  ledger.debit from amount

rebalance from:Account to:Account amount:Int
  need from != to else SameAccount
  keep ledger.total
  ensure ledger.balance' to = ledger.balance to + amount
  eg A:80@u1 B:0@u2 | rebalance A B 50 -> ok | A:30 B:50
body
  withdraw from amount
  deposit to amount

month_end from:Account to:Account house:Account amount:Int fee:Int
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount - fee
  emit TransferDone from to amount _
  emit TransferDone from house fee _
  emit Settled from to _
  eg A:500@u1 B:0@u1 H:0@u1 | month_end A B H 100 5 -> ok | A:395 B:100 H:5
body
  settle from to house amount fee
