import catalog.stock

const placed 1
const paid 2
const shipped 3
const cancelled 4

state phase Int
state item_of Int
state qty_of Int

invariant o:Int phase.get o >= 0
invariant o:Int phase.get o <= 4
invariant o:Int phase.get o = 0 or qty_of.get o > 0
invariant o:Int phase.get o = 0 or item_of.get o > 0

place order:Int item:Int qty:Int
  need order > 0 else BadOrder
  need item > 0 else NoSuchItem
  need phase.get order = 0 else OrderExists
  need price.get item > 0 else NotForSale
  need qty > 0 else BadQuantity
  need stock.get item >= qty else OutOfStock (qty - stock.get item)
  ok otherwise
  ensure phase.get' order = placed
  ensure item_of.get' order = item
  ensure qty_of.get' order = qty
  ensure stock.get' item = stock.get item - qty
  emit Placed order item qty
  eg stock.3:5 price.3:100 | place 1 3 2 -> ok | phase.1:1 item_of.1:3 qty_of.1:2 stock.3:3
  eg stock.3:5 price.3:100 phase.1:1 | place 1 3 2 -> OrderExists
  eg stock.3:1 price.3:100 | place 1 3 2 -> OutOfStock 1
  eg stock.3:5 | place 1 3 2 -> NotForSale
  eg stock.3:2 price.3:100 | place 1 3 2 -> ok | phase.1:1 stock.3:0
  eg stock.0:5 price.0:100 | place 1 0 2 -> NoSuchItem
  eg stock.3:5 price.3:100 | place 0 3 2 -> BadOrder
  eg stock.3:5 price.3:100 | place 1 3 0 -> BadQuantity
body
  take item qty
  phase.set order placed
  item_of.set order item
  qty_of.set order qty
  audit.emit (Placed order item qty)

advance order:Int
  need phase.get order = placed or phase.get order = paid else CannotAdvance
  ok otherwise
  ensure phase.get' order = phase.get order + 1
  eg phase.1:1 | advance 1 -> ok | phase.1:2
  eg phase.1:2 | advance 1 -> ok | phase.1:3
  eg phase.1:3 | advance 1 -> CannotAdvance
  eg | advance 1 -> CannotAdvance
body
  match phase.get order
    placed -> phase.set order paid
    paid -> phase.set order shipped

cancel order:Int
  need phase.get order = placed or phase.get order = paid else CannotCancel
  ok otherwise
  ensure phase.get' order = cancelled
  ensure stock.get' (item_of.get order) = stock.get (item_of.get order) + qty_of.get order
  emit Restocked (item_of.get order) (qty_of.get order)
  emit Cancelled order
  eg phase.1:1 item_of.1:3 qty_of.1:2 stock.3:5 | cancel 1 -> ok | phase.1:4 stock.3:7
  eg phase.1:3 | cancel 1 -> CannotCancel
body
  restock (item_of.get order) (qty_of.get order)
  phase.set order cancelled
  audit.emit (Cancelled order)
