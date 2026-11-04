state price Int
state stock Int

invariant i:Int price.get i >= 0
invariant i:Int stock.get i >= 0

set_price item:Int amount:Int
  need amount >= 0 else BadPrice
  ok otherwise
  ensure price.get' item = amount
  eg | set_price 7 30 -> ok | price.7:30
  eg | set_price 7 -1 -> BadPrice
body
  price.set item amount

restock item:Int n:Int
  need n > 0 else BadCount
  ok otherwise
  ensure stock.get' item = stock.get item + n
  eg stock.7:2 | restock 7 3 -> ok | stock.7:5
body
  stock.set item (stock.get item + n)

restock_range lo:Int hi:Int n:Int
  need n > 0 else BadCount
  ok otherwise
  ensure stock.get' j = stock.get j + n for j from lo to hi
  eg stock.1:2 stock.2:0 | restock_range 1 3 4 -> ok | stock.1:6 stock.2:4
body
  for k from lo to hi
    invariant stock.get' j = stock.get j + n for j from lo to k
    stock.set k (stock.get k + n)
