state status Int
state due Int

count_overdue lo:Int hi:Int -> Int
  ensure result >= 0
  ensure result <= hi - lo when hi >= lo
  eg status.1:1 due.1:3 status.2:1 due.2:9 status.3:2 due.3:1 now:5 | count_overdue 1 4 -> ok
body
  result = sum k from lo to hi of 1 where status.get k = 1 and due.get k < clock.now

report_overdue lo:Int hi:Int
  ok otherwise
  emit Overdue (count_overdue lo hi)
body
  audit.emit (Overdue (count_overdue lo hi))
