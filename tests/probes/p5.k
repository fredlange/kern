transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
body
  ledger.debit from amount
  ledger.credit to amount

pay_n buyer:Account seller:Account n:Int
  need n >= 0 else BadCount
  need buyer != seller else SameAccount
  ok otherwise
  keep ledger.total
body
  for k from 0 to n
    invariant ledger.total' = ledger.total
    transfer buyer seller 10
