state frozen Account
state spent Account
state limit Account
state hold Int
state disputed Int

invariant a:Account ledger.balance a >= 0
invariant o:Int hold.get o >= 0
invariant o:Int disputed.get o = 0 or hold.get o > 0

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
  need as transfer buyer escrow amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - amount
  ensure ledger.balance' escrow = ledger.balance escrow + amount
  emit as transfer buyer escrow amount
  emit Reserved buyer amount clock.now
  eg A:500@u1 E:0@u0 | reserve A E 100 -> ok | A:400 E:100
  eg A:50@u1 E:0@u0 | reserve A E 100 -> Insufficient 50
body
  transfer buyer escrow amount
  audit.emit (Reserved buyer amount clock.now)

release escrow:Account buyer:Account amount:Int
  need as transfer escrow buyer amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' buyer = ledger.balance buyer + amount
  emit as transfer escrow buyer amount
  emit Released buyer amount clock.now
  eg A:400@u1 E:100@u0 | release E A 100 -> ok | A:500 E:0
body
  transfer escrow buyer amount
  audit.emit (Released buyer amount clock.now)

capture escrow:Account seller:Account house:Account amount:Int
  need amount > 0 else BadAmount
  need escrow != seller and escrow != house and seller != house else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' seller = ledger.balance seller + amount when amount < 100
  ensure ledger.balance' seller = ledger.balance seller + amount - amount * 2 / 100 when amount >= 100
  ensure ledger.balance' house = ledger.balance house + amount * 2 / 100 when amount >= 100
  ensure ledger.balance' house = ledger.balance house when amount < 100
  emit Moved escrow seller amount clock.now when amount < 100
  emit Moved escrow seller (amount - amount * 2 / 100) clock.now when amount >= 100
  emit Moved escrow house (amount * 2 / 100) clock.now when amount >= 100
  emit Captured seller amount clock.now
  eg E:100@u0 B:0@u2 H:0@u9 | capture E B H 50 -> ok | E:50 B:50 H:0
  eg E:500@u0 B:0@u2 H:0@u9 | capture E B H 250 -> ok | E:250 B:245 H:5
  eg E:50@u0 B:0@u2 H:0@u9 | capture E B H 250 -> Insufficient 200
body
  if amount < 100
    transfer escrow seller amount
  else
    fee = amount * 2 / 100
    transfer escrow seller (amount - fee)
    transfer escrow house fee
  audit.emit (Captured seller amount clock.now)

purchase buyer:Account escrow:Account seller:Account house:Account amount:Int
  need as reserve buyer escrow amount
  need escrow != seller and escrow != house and seller != house else SameAccount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - amount when buyer != seller and buyer != house
  ensure ledger.balance' escrow = ledger.balance escrow
  emit as reserve buyer escrow amount
  emit as capture escrow seller house amount
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 | purchase A E B H 200 -> ok | A:300 E:0 B:196 H:4
body
  reserve buyer escrow amount
  capture escrow seller house amount

cancel escrow:Account buyer:Account amount:Int
  need as release escrow buyer amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer + amount
  emit as release escrow buyer amount
  emit Cancelled buyer clock.now
  eg A:400@u1 E:100@u0 | cancel E A 100 -> ok | A:500 E:0
body
  release escrow buyer amount
  audit.emit (Cancelled buyer clock.now)

refund escrow:Account buyer:Account seller:Account house:Account amount:Int part:Int
  need part > 0 and part <= amount else BadPart
  need escrow != buyer and escrow != seller and escrow != house and seller != house else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount when buyer != seller and buyer != house
  ensure ledger.balance' buyer = ledger.balance buyer + part when buyer != seller and buyer != house
  emit as release escrow buyer amount when part = amount
  emit Moved escrow buyer part clock.now when part < amount
  emit as capture escrow seller house (amount - part) when part < amount
  eg E:100@u0 A:0@u1 B:0@u2 H:0@u9 | refund E A B H 100 100 -> ok | E:0 A:100 B:0 H:0
  eg E:300@u0 A:0@u1 B:0@u2 H:0@u9 | refund E A B H 300 100 -> ok | E:0 A:100 B:196 H:4
body
  if part = amount
    release escrow buyer amount
  else
    transfer escrow buyer part
    capture escrow seller house (amount - part)

split escrow:Account a:Account b:Account amount:Int part:Int
  need part >= 0 and part <= amount else BadPart
  need escrow != a and escrow != b and a != b else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' a = ledger.balance a + part
  ensure ledger.balance' b = ledger.balance b + amount - part
  emit as move escrow a part
  emit as move escrow b (amount - part)
  eg E:100@u0 A:0@u1 B:0@u2 | split E A B 100 30 -> ok | E:0 A:30 B:70
  eg E:100@u0 A:0@u1 B:0@u2 | split E A B 100 100 -> ok | E:0 A:100 B:0
body
  move escrow a part
  move escrow b (amount - part)

tip buyer:Account seller:Account amount:Int
  need as move buyer seller amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' seller = ledger.balance seller + amount
  emit as move buyer seller amount
  emit Tipped seller amount clock.now
  eg A:10@u1 B:0@u2 | tip A B 0 -> ok | A:10 B:0
  eg A:10@u1 B:0@u2 | tip A B 5 -> ok | A:5 B:5
body
  move buyer seller amount
  audit.emit (Tipped seller amount clock.now)

open_order id:Int buyer:Account escrow:Account amount:Int
  need hold.get id = 0 else OrderExists
  need frozen.get buyer = 0 else Frozen
  need as reserve buyer escrow amount
  ok otherwise
  keep ledger.total
  ensure hold.get' id = amount
  ensure ledger.balance' escrow = ledger.balance escrow + amount
  ensure ledger.balance' buyer = ledger.balance buyer - amount
  emit as reserve buyer escrow amount
  emit Opened id amount clock.now
  eg A:500@u1 E:0@u0 | open_order 7 A E 100 -> ok | A:400 E:100 hold.7:100
  eg A:500@u1 E:0@u0 hold.7:5 | open_order 7 A E 100 -> OrderExists
  eg A:500@u1 E:0@u0 frozen.A:1 | open_order 7 A E 100 -> Frozen
body
  reserve buyer escrow amount
  hold.set id amount
  audit.emit (Opened id amount clock.now)

capture_order id:Int escrow:Account seller:Account house:Account
  need hold.get id > 0 else NoOrder
  need disputed.get id = 0 else Disputed
  need as capture escrow seller house (hold.get id)
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure ledger.balance' escrow = ledger.balance escrow - hold.get id
  ensure ledger.balance' seller = ledger.balance seller + hold.get id when hold.get id < 100
  ensure ledger.balance' seller = ledger.balance seller + hold.get id - hold.get id * 2 / 100 when hold.get id >= 100
  emit as capture escrow seller house (hold.get id)
  eg E:300@u0 B:0@u2 H:0@u9 hold.7:250 | capture_order 7 E B H -> ok | E:50 B:245 H:5 hold.7:0
  eg E:300@u0 B:0@u2 H:0@u9 hold.7:250 disputed.7:1 | capture_order 7 E B H -> Disputed
  eg E:300@u0 B:0@u2 H:0@u9 | capture_order 7 E B H -> NoOrder
body
  capture escrow seller house (hold.get id)
  hold.set id 0

cancel_order id:Int escrow:Account buyer:Account
  need hold.get id > 0 else NoOrder
  need disputed.get id = 0 else Disputed
  need as cancel escrow buyer (hold.get id)
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure ledger.balance' buyer = ledger.balance buyer + hold.get id
  emit as cancel escrow buyer (hold.get id)
  eg E:100@u0 A:0@u1 hold.3:100 | cancel_order 3 E A -> ok | E:0 A:100 hold.3:0
body
  cancel escrow buyer (hold.get id)
  hold.set id 0

refund_order id:Int escrow:Account buyer:Account seller:Account house:Account part:Int
  need hold.get id > 0 else NoOrder
  need disputed.get id = 0 else Disputed
  need as refund escrow buyer seller house (hold.get id) part
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure ledger.balance' buyer = ledger.balance buyer + part when buyer != seller and buyer != house
  emit as refund escrow buyer seller house (hold.get id) part
  eg E:300@u0 A:0@u1 B:0@u2 H:0@u9 hold.4:300 | refund_order 4 E A B H 100 -> ok | E:0 A:100 B:196 H:4 hold.4:0
body
  refund escrow buyer seller house (hold.get id) part
  hold.set id 0

open_dispute id:Int
  need hold.get id > 0 else NoOrder
  need disputed.get id = 0 else Disputed
  ok otherwise
  keep ledger.total
  ensure disputed.get' id = 1
  ensure hold.get' id = hold.get id
  emit DisputeOpened id clock.now
  eg hold.5:100 | open_dispute 5 -> ok | disputed.5:1
  eg | open_dispute 5 -> NoOrder
body
  disputed.set id 1
  audit.emit (DisputeOpened id clock.now)

resolve_for_buyer id:Int escrow:Account buyer:Account
  need disputed.get id = 1 else NotDisputed
  need as release escrow buyer (hold.get id)
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure disputed.get' id = 0
  ensure ledger.balance' buyer = ledger.balance buyer + hold.get id
  emit as release escrow buyer (hold.get id)
  emit Resolved id clock.now
  eg E:100@u0 A:0@u1 hold.5:100 disputed.5:1 | resolve_for_buyer 5 E A -> ok | E:0 A:100 hold.5:0 disputed.5:0
body
  release escrow buyer (hold.get id)
  hold.set id 0
  disputed.set id 0
  audit.emit (Resolved id clock.now)

resolve_for_seller id:Int escrow:Account seller:Account house:Account
  need disputed.get id = 1 else NotDisputed
  need as capture escrow seller house (hold.get id)
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure disputed.get' id = 0
  ensure ledger.balance' escrow = ledger.balance escrow - hold.get id
  emit as capture escrow seller house (hold.get id)
  emit Resolved id clock.now
  eg E:300@u0 B:0@u2 H:0@u9 hold.5:250 disputed.5:1 | resolve_for_seller 5 E B H -> ok | E:50 B:245 H:5 hold.5:0
body
  capture escrow seller house (hold.get id)
  hold.set id 0
  disputed.set id 0
  audit.emit (Resolved id clock.now)

resolve_split id:Int escrow:Account buyer:Account seller:Account part:Int
  need disputed.get id = 1 else NotDisputed
  need as split escrow buyer seller (hold.get id) part
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure disputed.get' id = 0
  ensure ledger.balance' buyer = ledger.balance buyer + part
  ensure ledger.balance' seller = ledger.balance seller + hold.get id - part
  emit as split escrow buyer seller (hold.get id) part
  emit Resolved id clock.now
  eg E:100@u0 A:0@u1 B:0@u2 hold.5:100 disputed.5:1 | resolve_split 5 E A B 40 -> ok | E:0 A:40 B:60 hold.5:0
body
  split escrow buyer seller (hold.get id) part
  hold.set id 0
  disputed.set id 0
  audit.emit (Resolved id clock.now)

set_limit a:Account n:Int
  need n >= 0 else BadLimit
  ok otherwise
  keep ledger.total
  ensure limit.get' a = n
  eg A:0@u1 | set_limit A 500 -> ok | limit.A:500
body
  limit.set a n

freeze a:Account
  ok otherwise
  keep ledger.total
  ensure frozen.get' a = 1
  emit Froze a clock.now
  eg A:0@u1 | freeze A -> ok | frozen.A:1
body
  frozen.set a 1
  audit.emit (Froze a clock.now)

unfreeze a:Account
  ok otherwise
  keep ledger.total
  ensure frozen.get' a = 0
  emit Unfroze a clock.now
  eg A:0@u1 frozen.A:1 | unfreeze A -> ok | frozen.A:0
body
  frozen.set a 0
  audit.emit (Unfroze a clock.now)

withdraw a:Account bank:Account amount:Int
  need frozen.get a = 0 else Frozen
  need spent.get a + amount <= limit.get a else OverLimit (spent.get a + amount - limit.get a)
  need as transfer a bank amount
  ok otherwise
  keep ledger.total
  ensure spent.get' a = spent.get a + amount
  ensure ledger.balance' a = ledger.balance a - amount
  ensure ledger.balance' bank = ledger.balance bank + amount
  emit as transfer a bank amount
  eg A:500@u1 K:0@u9 limit.A:300 | withdraw A K 200 -> ok | A:300 K:200 spent.A:200
  eg A:500@u1 K:0@u9 limit.A:300 spent.A:200 | withdraw A K 200 -> OverLimit 100
body
  transfer a bank amount
  spent.set a (spent.get a + amount)

reset_day a:Account
  ok otherwise
  keep ledger.total
  ensure spent.get' a = 0
  eg A:0@u1 spent.A:70 | reset_day A -> ok | spent.A:0
body
  spent.set a 0

payout_seller seller:Account bank:Account amount:Int
  need as withdraw seller bank amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' seller = ledger.balance seller - amount
  ensure spent.get' seller = spent.get seller + amount
  emit as withdraw seller bank amount
  emit PaidOut seller amount clock.now
  eg B:500@u2 K:0@u9 limit.B:1000 | payout_seller B K 200 -> ok | B:300 K:200 spent.B:200
body
  withdraw seller bank amount
  audit.emit (PaidOut seller amount clock.now)

deposit bank:Account a:Account amount:Int
  need as transfer bank a amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' a = ledger.balance a + amount
  emit as transfer bank a amount
  emit Deposited a amount clock.now
  eg K:900@u9 A:0@u1 | deposit K A 100 -> ok | K:800 A:100
body
  transfer bank a amount
  audit.emit (Deposited a amount clock.now)

purchase_order id:Int buyer:Account escrow:Account seller:Account house:Account amount:Int
  need as open_order id buyer escrow amount
  need disputed.get id = 0 else Disputed
  need escrow != seller and escrow != house and seller != house else SameAccount
  ok otherwise
  keep ledger.total
  ensure hold.get' id = 0
  ensure ledger.balance' escrow = ledger.balance escrow
  ensure ledger.balance' buyer = ledger.balance buyer - amount when buyer != seller and buyer != house
  emit as open_order id buyer escrow amount
  emit as capture escrow seller house amount
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 | purchase_order 9 A E B H 250 -> ok | A:250 E:0 B:245 H:5 hold.9:0
body
  open_order id buyer escrow amount
  capture_order id escrow seller house
