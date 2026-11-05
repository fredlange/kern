state hold Int
state flag Account
state seen Account
invariant o:Int hold.get o >= 0
invariant a:Account flag.get a >= 0 and ledger.balance a >= 0

put k:Int v:Int
  need v >= 0 else Neg
body
  hold.set (k + 1) (v - 5)

restore k:Int
  ensure hold.get' (k + 1) = hold.get (k + 1)
body
  x = hold.get (k + 1)
  hold.set (k + 1) (x + 1)
  hold.set (k + 1) x

relay k:Int v:Int
  need v >= 0 else Neg
body
  put k v

sweep lo:Int hi:Int
body
  for j from lo to hi
    hold.set j (hold.get j - 1)

late k:Int
body
  hold.set (k + 1) (0 - 1)
  need k > 1000 else TooSmall

mark a:Account v:Int
  ensure flag.get' a = v
body
  flag.set a v

safe a:Account
  ensure flag.get' a = 1
body
  flag.set a 1

note a:Account
  ensure seen.get' a = 1
body
  seen.set a 1

pay a:Account b:Account n:Int
  need a != b else Same
  need n > 0 else BadAmount
body
  ledger.debit a n
  ledger.credit b n
