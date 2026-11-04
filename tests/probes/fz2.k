state hold Int
state tally Int
state aux Int
invariant o:Int hold.get o >= 0

f lo:Int hi:Int other:Int
  need 0 <= (hi - 1) else Empty
  need other < 0 or other >= (hi - 1) else InRange
  ok otherwise
  ensure hold.get' j = 7 when hold.get j > 0 for j from 0 to (hi - 1)
  ensure hold.get' j = hold.get j when hold.get j <= 0 for j from 0 to (hi - 1)
  ensure aux.get' j <= hold.get j for j from 0 to (hi - 1)
  ensure hold.get' other = hold.get other
body
  for k from 0 to (hi - 1)
    invariant hold.get' j = 7 when hold.get j > 0 for j from 0 to k
    invariant aux.get' j = hold.get j for j from 0 to k
    if hold.get k > 0
      hold.set k 7
    aux.set k (hold.get k)
