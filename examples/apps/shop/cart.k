import catalog.stock

const max_lines 10

state cart Int -> List

invariant c:Int length (cart.get c) <= max_lines

add_line c:Int item:Int
  need c > 0 else BadCart
  need item > 0 else NoSuchItem
  need price.get item > 0 else NotForSale
  need length (cart.get c) < max_lines else CartFull
  ok otherwise
  ensure cart.get' c = append (cart.get c) item
  ensure length (cart.get' c) = length (cart.get c) + 1
  ensure at (cart.get' c) (length (cart.get c)) = item
  eg price.3:100 | add_line 1 3 -> ok | cart.1:[3]
  eg price.3:100 cart.1:[3] | add_line 1 3 -> ok | cart.1:[3 3]
  eg | add_line 1 3 -> NotForSale
  eg price.3:100 | add_line 0 3 -> BadCart
  eg price.0:100 | add_line 1 0 -> NoSuchItem
  eg price.3:100 cart.1:[1 1 1 1 1 1 1 1 1 1] | add_line 1 3 -> CartFull
  eg price.3:100 cart.1:[1 1 1 1 1 1 1 1 1] | add_line 1 3 -> ok
body
  cart.set c (append (cart.get c) item)

empty_cart c:Int
  need c > 0 else BadCart
  ok otherwise
  ensure cart.get' c = []
  eg cart.1:[3 4] | empty_cart 1 -> ok | cart.1:[]
  eg | empty_cart 0 -> BadCart
body
  cart.set c []

lines c:Int -> Int
  ensure result = length (cart.get c)
  ensure result >= 0
  eg cart.1:[3 4] | lines 1 -> ok
body
  result = length (cart.get c)

cart_value c:Int -> Int
  ensure result = 0 when length (cart.get c) = 0
  eg cart.2:[] | cart_value 2 -> ok
  eg price.3:100 price.4:50 cart.1:[3 4 3] | cart_value 1 -> ok
body
  result = sum i from 0 to (length (cart.get c)) of price.get (at (cart.get c) i)

line_item c:Int i:Int -> Int
  ensure result = 0 when i < 0 or i >= length (cart.get c)
  eg cart.1:[7 8 9] | line_item 1 1 -> ok
  eg cart.1:[7 8 9] | line_item 1 3 -> ok
  eg cart.1:[7 8 9] | line_item 1 -1 -> ok
body
  if i >= 0 and i < length (cart.get c)
    result = at (cart.get c) i
  else
    result = 0
