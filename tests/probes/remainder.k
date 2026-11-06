const days 7
const slots 3 * days + 1
state out Int
state price Int -> Decimal

rem a:Int -> Int
  ensure result >= 0 and result < 7
  ensure a = (a / 7) * 7 + result
  eg | rem -1 -> ok 6
  eg | rem 15 -> ok 1
body
  result = a % days

rem_bad a:Int -> Int
  ensure result <= 0 when a < 0
body
  result = a % 7

trunc_claim -> Int
  ensure result = 0 - 1
body
  result = (0 - 8) % 7

slot r:Int d:Int
  need d >= 0 and d < days else Bad
  ok otherwise
  ensure out.get' (r * days + d) = slots
body
  out.set (r * days + d) slots

dsum k:Int -> Decimal
  ensure result = price.get k + price.get (k + 1)
body
  result = sum j from k to (k + 2) of price.get j

dint n:Int -> Decimal
  ensure result = n
  eg | dint 3 -> ok 3.0
body
  result = n
