state hold Int
invariant o:Int hold.get o >= 0

bump lo:Int hi:Int
  ensure hold.get' j = hold.get j + 2 for j from lo to hi
body
  for k from lo to hi
    invariant hold.get' j = hold.get j + 2 for j from lo to k
    hold.set k (hold.get k + 1)
