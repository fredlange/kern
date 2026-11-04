import money.ledger
import inventory.catalog

buy buyer:Account shop:Account item:Int
  need stock.get item > 0 else OutOfStock
  need as transfer buyer shop (price.get item)
  ok otherwise
  keep ledger.total
  ensure stock.get' item = stock.get item - 1
  ensure ledger.balance' buyer = ledger.balance buyer - price.get item
  emit as transfer buyer shop (price.get item)
  emit Sold item buyer clock.now
  eg A:100@u1 S:0@u2 price.7:30 stock.7:2 | buy A S 7 -> ok | A:70 S:30 stock.7:1
  eg A:10@u1 S:0@u2 price.7:30 stock.7:2 | buy A S 7 -> Insufficient 20
  eg A:100@u1 S:0@u2 price.7:30 | buy A S 7 -> OutOfStock
body
  transfer buyer shop (price.get item)
  stock.set item (stock.get item - 1)
  audit.emit (Sold item buyer clock.now)

refund buyer:Account shop:Account item:Int
  need as transfer shop buyer (price.get item)
  ok otherwise
  keep ledger.total
  ensure stock.get' item = stock.get item + 1
  emit as transfer shop buyer (price.get item)
  emit Returned item buyer clock.now
  eg A:0@u1 S:30@u2 price.7:30 stock.7:1 | refund A S 7 -> ok | A:30 S:0 stock.7:2
body
  transfer shop buyer (price.get item)
  stock.set item (stock.get item + 1)
  audit.emit (Returned item buyer clock.now)
