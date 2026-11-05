const fee 5
const cap 3
const lim 100
const two 2
const unused 7

state hold Int

half n:Int -> Int
  ensure result * two <= n
body
  result = n / two

charge a:Account k:Int amount:Int
  need amount > fee else TooSmall fee
  need hold.get k + amount <= lim else Over (lim - hold.get k)
  need ledger.balance a >= amount else Insufficient amount
  ensure ledger.balance' a = ledger.balance a - amount
  ensure hold.get' k = hold.get k + amount - fee
  emit Charged a amount fee
  eg A:50@u1 hold.1:0 | charge A 1 10 -> ok | A:40 hold.1:5
  eg A:50@u1 | charge A 1 5 -> TooSmall 5
  eg A:500@u1 hold.1:98 | charge A 1 10 -> Over 2
body
  ledger.debit a amount
  net = amount - fee
  hold.set k (hold.get k + net)
  audit.emit (Charged a amount fee)

relay a:Account k:Int amount:Int
  need as charge a k amount
  emit as charge a k amount
body
  charge a k amount

bump lo:Int
  ok otherwise
  ensure hold.get' j = hold.get j + 1 when hold.get j < lim for j from lo to (lo + cap)
  ensure hold.get' j = hold.get j when hold.get j >= lim for j from lo to (lo + cap)
body
  for k from lo to (lo + cap)
    invariant hold.get' j = hold.get j + 1 when hold.get j < lim for j from lo to k
    invariant hold.get' j = hold.get j when hold.get j >= lim for j from lo to k
    if hold.get k < lim
      hold.set k (hold.get k + 1)

scaled x:Int -> Int
  ensure result = x * fee + half x
body
  result = fee * x + half x
