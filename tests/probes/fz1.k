state hold Int
state tally Int
state aux Int
invariant o:Int hold.get o >= 0

f lo:Int hi:Int
  need lo <= hi else Empty
  ok otherwise
  ensure hold.get' j = 5 when hold.get j > 0 for j from lo to hi
  ensure hold.get' j = hold.get j when hold.get j <= 0 for j from (lo - 1) to hi
  ensure ledger.total' = ledger.total
body
  hold.set lo 5
  for k from lo to hi
    invariant hold.get' j = 6 when hold.get j > 0 for j from lo to k
    invariant hold.get' j = hold.get j when hold.get j <= 0 for j from lo to k
    invariant ledger.total' = ledger.total
    if hold.get k > 0
      hold.set k 5
