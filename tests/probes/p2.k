state hold Int
invariant o:Int hold.get o >= 0

shifted lo:Int hi:Int other:Int
  need other >= hi else InRange
  ensure hold.get' other = hold.get other
body
  for k from lo to hi
    invariant hold.get' j = 0 for j from lo to k
    hold.set (k + 1) 0
