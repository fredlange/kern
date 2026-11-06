state price Int -> Decimal
state cnt Int

mul a:Decimal b:Decimal -> Decimal
  ensure result <= a * b
  ensure result = a * b
body
  result = a * b

neg_half -> Decimal
  ensure result = 0 - 0.0001
  eg | neg_half -> ok -0.0001
body
  result = (0 - 0.0001) * 0.5

neg_half_bad -> Decimal
  ensure result = 0
body
  result = (0 - 0.0001) * 0.5

third x:Decimal -> Decimal
  ensure result * 3 <= x
  ensure x - result * 3 < 0.0003
body
  result = x / 3

third_bad x:Decimal -> Decimal
  ensure result * 3 = x
body
  result = x / 3

neg_third -> Decimal
  ensure result = 0 - 0.3334
  eg | neg_third -> ok -0.3334
body
  result = (0 - 1.0) / 3

promote n:Int -> Decimal
  ensure result = n
  ensure result = n * 1.0
  eg | promote 3 -> ok 3
body
  result = n + 0.0

one_eq -> Int
  ensure result = 1
body
  if 1 = 1.0
    result = 1
  else
    result = 0

exact -> Decimal
  ensure result = 0.25
  ensure result != 0.2501
body
  result = 0.5 * 0.5

lossy -> Decimal
  ensure result = 1
body
  result = 0.3333 * 3.0

store k:Int p:Decimal
  ok otherwise
  ensure price.get' k = p + price.get k
  eg price.4:2.5 | store 4 1.25 -> ok | price.4:3.75
  eg | store 4 2 -> ok | price.4:2
body
  price.set k (p + price.get k)

sq_nonneg a:Decimal -> Decimal
  ensure result >= 0
body
  result = a * a
