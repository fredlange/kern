state hold Int
invariant o:Int hold.get o >= 0

liar lo:Int hi:Int
  ensure hold.get' j = 99 for j from lo to hi
body
  for k from lo to hi
    invariant 1 = 0
    hold.set k 1
