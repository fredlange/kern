state hold Int
state expiry Int
invariant o:Int hold.get o >= 0

zero_all lo:Int hi:Int
  ensure hold.get' j = 0 for j from lo to hi
body
  for k from lo to hi
    invariant hold.get' j = 0 when expiry.get j < clock.now for j from lo to k
    if expiry.get k < clock.now
      hold.set k 0
