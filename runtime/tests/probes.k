handler rand.below n:Int -> Int
  need n > 0 else BadBound
  ensure result >= 0 and result < n

state slot Int
state mark Account

late a:Account b:Account amount:Int k:Int
body
  slot.set k amount
  mark.set a 1
  ledger.debit a 1
  ledger.credit b 1
  need ledger.balance a >= amount else Short amount (ledger.balance a) k

outer a:Account b:Account amount:Int k:Int
body
  slot.set (k + 1) 5
  mark.set b 2
  late a b amount k

snapshot k:Int
body
  slot.set k ledger.total

record x:Int y:Int
body
  slot.set 1 x
  slot.set 2 y

draw2 n:Int
body
  record (rand.below 3) (rand.below n)

pick a:Account b:Account
body
  if ledger.owner a = ledger.owner b
    audit.emit (Same (ledger.owner a) a b)
  else
    audit.emit (Differ a (ledger.balance a - ledger.balance b) (ledger.balance a > ledger.balance b))

half n:Int -> Int
body
  result = (n - 7) / 2

use_half a:Account n:Int
body
  mark.set a (half n)
  for i from 0 to 3
    slot.set (i + n) (half (n + i))
