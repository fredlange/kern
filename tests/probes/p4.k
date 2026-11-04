state hold Int
invariant o:Int hold.get o >= 0

drain lo:Int hi:Int
  ok otherwise
body
  for k from lo to hi
    invariant 1 = 1
    hold.set k (hold.get k - 1)
