transfer from:Account to:Account amount:Int
  need amount > 0 else BadAmount
  need from != to else SameAccount
  need ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit Moved from to amount clock.now
  eg A:500@u1 B:0@u2 | transfer A B 100 -> ok | A:400 B:100
  eg A:50@u1 B:0@u2 | transfer A B 100 -> Insufficient 50
  eg A:500@u1 B:0@u2 | transfer A A 100 -> SameAccount
  eg A:500@u1 B:0@u2 | transfer A B 0 -> BadAmount
body
  ledger.debit from amount
  ledger.credit to amount
  audit.emit (Moved from to amount clock.now)

move from:Account to:Account amount:Int
  need amount >= 0 else BadAmount
  need amount = 0 or from != to else SameAccount
  need amount = 0 or ledger.balance from >= amount else Insufficient (amount - ledger.balance from)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' from = ledger.balance from - amount
  ensure ledger.balance' to = ledger.balance to + amount
  emit Moved from to amount clock.now when amount > 0
  eg A:500@u1 B:0@u2 | move A B 0 -> ok | A:500 B:0
  eg A:500@u1 B:0@u2 | move A B 100 -> ok | A:400 B:100
  eg A:0@u1 | move A A 0 -> ok | A:0
body
  if amount > 0
    transfer from to amount

reserve buyer:Account escrow:Account amount:Int
  need amount > 0 else BadAmount
  need buyer != escrow else SameAccount
  need ledger.balance buyer >= amount else Insufficient (amount - ledger.balance buyer)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - amount
  ensure ledger.balance' escrow = ledger.balance escrow + amount
  emit Moved buyer escrow amount clock.now
  emit Reserved buyer amount clock.now
  eg A:500@u1 E:0@u0 | reserve A E 100 -> ok | A:400 E:100
  eg A:50@u1 E:0@u0 | reserve A E 100 -> Insufficient 50
body
  transfer buyer escrow amount
  audit.emit (Reserved buyer amount clock.now)

release escrow:Account buyer:Account amount:Int
  need amount > 0 else BadAmount
  need escrow != buyer else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' buyer = ledger.balance buyer + amount
  emit Moved escrow buyer amount clock.now
  emit Released buyer amount clock.now
  eg A:400@u1 E:100@u0 | release E A 100 -> ok | A:500 E:0
body
  transfer escrow buyer amount
  audit.emit (Released buyer amount clock.now)

house_fee amount:Int fee:Int -> Int
  ensure result = 50 when amount >= 1000 and fee > 50
  ensure result = fee when amount < 1000 or fee <= 50
body
  if amount >= 1000 and fee > 50
    result = 50
  else
    result = fee

capture escrow:Account seller:Account house:Account amount:Int fee:Int
  need amount > 0 else BadAmount
  need fee >= 0 and fee <= amount else BadFee
  need escrow != seller and escrow != house and seller != house else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' seller = ledger.balance seller + amount when amount < 100
  ensure ledger.balance' seller = ledger.balance seller + amount - house_fee amount fee when amount >= 100
  ensure ledger.balance' house = ledger.balance house + house_fee amount fee when amount >= 100
  ensure ledger.balance' house = ledger.balance house when amount < 100
  emit Moved escrow seller amount clock.now when amount < 100
  emit Moved escrow seller (amount - house_fee amount fee) clock.now when amount >= 100 and amount - house_fee amount fee > 0
  emit Moved escrow house (house_fee amount fee) clock.now when amount >= 100 and house_fee amount fee > 0
  emit Captured seller amount clock.now
  eg E:100@u0 B:0@u2 H:0@u9 | capture E B H 50 5 -> ok | E:50 B:50 H:0
  eg E:500@u0 B:0@u2 H:0@u9 | capture E B H 200 10 -> ok | E:300 B:190 H:10
  eg E:500@u0 B:0@u2 H:0@u9 | capture E B H 200 300 -> BadFee
  eg E:2000@u0 B:0@u2 H:0@u9 | capture E B H 1000 80 -> ok | E:1000 B:950 H:50
  eg E:2000@u0 B:0@u2 H:0@u9 | capture E B H 1000 30 -> ok | E:1000 B:970 H:30
body
  if amount < 100
    transfer escrow seller amount
  else
    f = house_fee amount fee
    move escrow seller (amount - f)
    move escrow house f
  audit.emit (Captured seller amount clock.now)

purchase buyer:Account escrow:Account seller:Account house:Account amount:Int fee:Int
  need amount > 0 else BadAmount
  need fee >= 0 and fee <= amount else BadFee
  need buyer != escrow and escrow != seller and escrow != house and seller != house else SameAccount
  need ledger.balance buyer >= amount else Insufficient (amount - ledger.balance buyer)
  need ledger.balance escrow >= 0 else Overdrawn
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - amount when buyer != seller and buyer != house
  ensure ledger.balance' escrow = ledger.balance escrow
  emit Moved buyer escrow amount clock.now
  emit Reserved buyer amount clock.now
  emit Moved escrow seller amount clock.now when amount < 100
  emit Moved escrow seller (amount - house_fee amount fee) clock.now when amount >= 100 and amount - house_fee amount fee > 0
  emit Moved escrow house (house_fee amount fee) clock.now when amount >= 100 and house_fee amount fee > 0
  emit Captured seller amount clock.now
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 | purchase A E B H 200 10 -> ok | A:300 E:0 B:190 H:10
body
  reserve buyer escrow amount
  capture escrow seller house amount fee

cancel escrow:Account buyer:Account amount:Int
  need amount > 0 else BadAmount
  need escrow != buyer else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer + amount
  emit Moved escrow buyer amount clock.now
  emit Released buyer amount clock.now
  emit Cancelled buyer clock.now
  eg A:400@u1 E:100@u0 | cancel E A 100 -> ok | A:500 E:0
body
  release escrow buyer amount
  audit.emit (Cancelled buyer clock.now)

refund escrow:Account buyer:Account seller:Account house:Account amount:Int part:Int fee:Int
  need part > 0 and part <= amount else BadPart
  need part = amount or fee >= 0 and fee <= amount - part else BadFee
  need escrow != buyer and escrow != seller and escrow != house and seller != house else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount when part = amount
  ensure ledger.balance' escrow = ledger.balance escrow - amount when part < amount and buyer != seller and buyer != house
  ensure ledger.balance' buyer = ledger.balance buyer + part when buyer != seller and buyer != house
  eg E:100@u0 A:0@u1 B:0@u2 H:0@u9 | refund E A B H 100 100 0 -> ok | E:0 A:100 B:0 H:0
  eg E:300@u0 A:0@u1 B:0@u2 H:0@u9 | refund E A B H 300 100 10 -> ok | E:0 A:100 B:190 H:10
body
  if part = amount
    release escrow buyer amount
  else
    move escrow buyer part
    capture escrow seller house (amount - part) fee

split escrow:Account a:Account b:Account amount:Int part:Int
  need part >= 0 and part <= amount else BadPart
  need escrow != a and escrow != b and a != b else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' a = ledger.balance a + part
  ensure ledger.balance' b = ledger.balance b + amount - part
  eg E:100@u0 A:0@u1 B:0@u2 | split E A B 100 30 -> ok | E:0 A:30 B:70
  eg E:100@u0 A:0@u1 B:0@u2 | split E A B 100 100 -> ok | E:0 A:100 B:0
body
  move escrow a part
  move escrow b (amount - part)

tip buyer:Account seller:Account amount:Int
  need amount >= 0 else BadAmount
  need amount = 0 or buyer != seller else SameAccount
  need amount = 0 or ledger.balance buyer >= amount else Insufficient (amount - ledger.balance buyer)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' seller = ledger.balance seller + amount
  emit Moved buyer seller amount clock.now when amount > 0
  emit Tipped seller amount clock.now
  eg A:10@u1 B:0@u2 | tip A B 0 -> ok | A:10 B:0
  eg A:10@u1 B:0@u2 | tip A B 5 -> ok | A:5 B:5
body
  move buyer seller amount
  audit.emit (Tipped seller amount clock.now)
