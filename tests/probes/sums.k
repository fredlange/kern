state hold Int
state status Int

total lo:Int hi:Int -> Int
  ensure result = sum j from lo to hi of hold.get j
body
  result = sum j from lo to hi of hold.get j

count lo:Int hi:Int -> Int
  ensure result >= 0
  ensure result <= hi - lo when hi >= lo
body
  result = sum j from lo to hi of 1 where status.get j = 1

add_in2 k:Int v:Int lo:Int hi:Int
  ok otherwise
  ensure sum j from lo to hi of hold.get' j = (sum j from lo to hi of hold.get j) + v when lo <= k and k < hi
  ensure sum j from lo to hi of hold.get' j = sum j from lo to hi of hold.get j when k < lo or k >= hi
body
  hold.set k (hold.get k + v)

after_write k:Int
  ok otherwise
  ensure hold.get' 1000 = hold.get k + 1 when k != 1000
body
  hold.set k (hold.get k + 1)
  x = total k (k + 1)
  hold.set 1000 x

empty lo:Int
  ok otherwise
  ensure hold.get' 5 = 0
body
  hold.set 5 (sum j from lo to lo of hold.get j + 7)

reverse lo:Int hi:Int
  ok otherwise
  ensure hold.get' 5 = 0 when hi <= lo
body
  hold.set 5 (sum j from lo to hi of 3)

timed lo:Int hi:Int -> Int
  ensure result = sum j from lo to hi of 1 where hold.get j < clock.now
body
  result = sum j from lo to hi of 1 where hold.get j < clock.now

bump k:Int
  ok otherwise
  ensure hold.get' k = hold.get k + 1
body
  hold.set k (hold.get k + 1)

pre_ok
  ok otherwise
  ensure hold.get' 9 = total 0 3
body
  x = total 0 3
  hold.set 0 100
  hold.set 9 x

via_callee
  ok otherwise
  ensure hold.get' 9 = total 0 3 + 1
body
  bump 0
  x = total 0 3
  hold.set 9 x

guard k:Int
  need total 0 3 < 10 else Full
  ok otherwise
  ensure hold.get' k = hold.get k + 1
body
  need total 0 3 < 10 else Full
  hold.set k (hold.get k + 1)

guard_late k:Int
  ok otherwise
body
  hold.set k (hold.get k + 1)
  need total 0 3 < 10 else Full
