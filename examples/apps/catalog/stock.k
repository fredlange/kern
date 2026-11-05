state stock Int
state price Int

invariant i:Int stock.get i >= 0
invariant i:Int price.get i >= 0

restock item:Int qty:Int
  need item > 0 else NoSuchItem
  need qty > 0 else BadQuantity
  ok otherwise
  ensure stock.get' item = stock.get item + qty
  emit Restocked item qty
  eg stock.3:4 | restock 3 6 -> ok | stock.3:10
  eg | restock 0 6 -> NoSuchItem
  eg | restock 3 0 -> BadQuantity
body
  stock.set item (stock.get item + qty)
  audit.emit (Restocked item qty)

take item:Int qty:Int
  need qty > 0 else BadQuantity
  need stock.get item >= qty else OutOfStock (qty - stock.get item)
  ok otherwise
  ensure stock.get' item = stock.get item - qty
  eg stock.3:4 | take 3 4 -> ok | stock.3:0
  eg stock.3:4 | take 3 5 -> OutOfStock 1
  eg | take 3 0 -> BadQuantity
body
  stock.set item (stock.get item - qty)

set_price item:Int cents:Int
  need item > 0 else NoSuchItem
  need cents > 0 else BadPrice
  ok otherwise
  ensure price.get' item = cents
  eg | set_price 3 250 -> ok | price.3:250
  eg | set_price 3 0 -> BadPrice
  eg | set_price 0 250 -> NoSuchItem
  eg | set_price 1 250 -> ok | price.1:250
body
  price.set item cents
