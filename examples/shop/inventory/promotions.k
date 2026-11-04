import inventory.catalog

discount_range lo:Int hi:Int
  ok otherwise
  ensure price.get' j = price.get j * 9 / 10 for j from lo to hi
  eg price.1:100 price.2:35 | discount_range 1 3 -> ok | price.1:90 price.2:31
body
  for k from lo to hi
    invariant price.get' j = price.get j * 9 / 10 for j from lo to k
    price.set k (price.get k * 9 / 10)
