state hold Int
invariant o:Int hold.get o >= 0

touch_first lo:Int hi:Int
  ensure hold.get' lo = 7
body
  for k from lo to hi
    invariant hold.get' j = 7 for j from lo to k
    hold.set k 7
