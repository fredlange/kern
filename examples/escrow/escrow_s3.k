state frozen Account
state spent Account
state limit Account
state hold Int
state disputed Int
state verified Account
state points Account
state sub_amount Int
state sub_due Int
state paused Int
state captured Int

invariant a:Account ledger.balance a >= 0
invariant o:Int hold.get o >= 0
invariant o:Int disputed.get o = 0 or hold.get o > 0
invariant o:Int sub_amount.get o >= 0
invariant o:Int captured.get o >= 0
invariant a:Account points.get a >= 0

fee_for amount:Int -> Int
  ensure result = 0 when amount < 100
  ensure result = amount * 2 / 100 when amount >= 100 and amount < 1000
  ensure result = amount / 100 + 10 when amount >= 1000
  eg | fee_for 250 -> ok
body
  if amount < 100
    result = 0
  else
    if amount < 1000
      result = amount * 2 / 100
    else
      result = amount / 100 + 10

vat_of amount:Int -> Int
  ensure result = amount / 4
  eg | vat_of 100 -> ok
body
  result = amount / 4

points_for amount:Int -> Int
  ensure result = amount / 100 when amount >= 0
  ensure result = 0 when amount < 0
  eg | points_for 250 -> ok
body
  if amount >= 0
    result = amount / 100
  else
    result = 0

holdback_for amount:Int -> Int
  ensure result = amount / 10
  eg | holdback_for 250 -> ok
body
  result = amount / 10

late_fee days:Int -> Int
  ensure result = 0 when days <= 0
  ensure result = days * 5 when days > 0 and days <= 10
  ensure result = 50 when days > 10
  eg | late_fee 3 -> ok
body
  if days <= 0
    result = 0
  else
    if days <= 10
      result = days * 5
    else
      result = 50


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
  need distinct escrow seller house else SameAccount
  need ledger.balance escrow >= amount else Insufficient (amount - ledger.balance escrow)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' escrow = ledger.balance escrow - amount
  ensure ledger.balance' seller = ledger.balance seller + amount - fee_for amount
  ensure ledger.balance' house = ledger.balance house + fee_for amount
  emit Moved escrow seller amount clock.now when fee_for amount = 0
  emit Moved escrow seller (amount - fee_for amount) clock.now when fee_for amount > 0
  emit Moved escrow house (fee_for amount) clock.now when fee_for amount > 0
  emit Captured seller amount clock.now
  eg E:100@u0 B:0@u2 H:0@u9 | capture E B H 50 -> ok | E:50 B:50 H:0
  eg E:500@u0 B:0@u2 H:0@u9 | capture E B H 250 -> ok | E:250 B:245 H:5
  eg E:50@u0 B:0@u2 H:0@u9 | capture E B H 250 -> Insufficient 200
  eg E:5000@u0 B:0@u2 H:0@u9 | capture E B H 2000 -> ok | E:3000 B:1970 H:30
body
  fee = fee_for amount
  if fee = 0
    transfer escrow seller amount
  else
    transfer escrow seller (amount - fee)
    transfer escrow house fee
  audit.emit (Captured seller amount clock.now)

purchase buyer:Account escrow:Account seller:Account house:Account amount:Int
  need as reserve buyer escrow amount
  need distinct escrow seller house else SameAccount
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
  need distinct escrow buyer seller house else SameAccount
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
  need distinct escrow a b else SameAccount
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
  ensure ledger.balance' buyer = ledger.balance buyer - amount
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
  ensure ledger.balance' seller = ledger.balance seller + hold.get id - fee_for (hold.get id)
  emit as capture escrow seller house (hold.get id)
  ensure captured.get' id = hold.get id
  eg E:300@u0 B:0@u2 H:0@u9 hold.7:250 | capture_order 7 E B H -> ok | E:50 B:245 H:5 hold.7:0 captured.7:250
  eg E:300@u0 B:0@u2 H:0@u9 hold.7:250 disputed.7:1 | capture_order 7 E B H -> Disputed
  eg E:300@u0 B:0@u2 H:0@u9 | capture_order 7 E B H -> NoOrder
body
  captured.set id (hold.get id)
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
  need verified.get a = 1 else Unverified
  need spent.get a + amount <= limit.get a else OverLimit (spent.get a + amount - limit.get a)
  need as transfer a bank amount
  ok otherwise
  keep ledger.total
  ensure spent.get' a = spent.get a + amount
  ensure ledger.balance' a = ledger.balance a - amount
  ensure ledger.balance' bank = ledger.balance bank + amount
  emit as transfer a bank amount
  eg A:500@u1 K:0@u9 limit.A:300 verified.A:1 | withdraw A K 200 -> ok | A:300 K:200 spent.A:200
  eg A:500@u1 K:0@u9 limit.A:300 spent.A:200 verified.A:1 | withdraw A K 200 -> OverLimit 100
  eg A:500@u1 K:0@u9 limit.A:300 | withdraw A K 200 -> Unverified
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
  eg B:500@u2 K:0@u9 limit.B:1000 verified.B:1 | payout_seller B K 200 -> ok | B:300 K:200 spent.B:200
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
  need distinct escrow seller house else SameAccount
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

verify a:Account
  ok otherwise
  keep ledger.total
  ensure verified.get' a = 1
  emit Verified a clock.now
  eg A:0@u1 | verify A -> ok | verified.A:1
body
  verified.set a 1
  audit.emit (Verified a clock.now)

unverify a:Account
  ok otherwise
  keep ledger.total
  ensure verified.get' a = 0
  emit Unverified a clock.now
  eg A:0@u1 verified.A:1 | unverify A -> ok | verified.A:0
body
  verified.set a 0
  audit.emit (Unverified a clock.now)

earn_points a:Account amount:Int
  ok otherwise
  keep ledger.total
  ensure points.get' a = points.get a + points_for amount
  eg A:0@u1 points.A:3 | earn_points A 250 -> ok | points.A:5
body
  points.set a (points.get a + points_for amount)

redeem_points a:Account house:Account n:Int
  need points.get a >= n else NotEnoughPoints (n - points.get a)
  need as transfer house a n
  ok otherwise
  keep ledger.total
  ensure points.get' a = points.get a - n
  ensure ledger.balance' a = ledger.balance a + n
  emit as transfer house a n
  emit Redeemed a n clock.now
  eg A:0@u1 H:100@u9 points.A:30 | redeem_points A H 20 -> ok | A:20 H:80 points.A:10
  eg A:0@u1 H:100@u9 points.A:5 | redeem_points A H 20 -> NotEnoughPoints 15
body
  transfer house a n
  points.set a (points.get a - n)
  audit.emit (Redeemed a n clock.now)

clear_points a:Account
  ok otherwise
  keep ledger.total
  ensure points.get' a = 0
  eg A:0@u1 points.A:9 | clear_points A -> ok | points.A:0
body
  points.set a 0

transfer_points a:Account b:Account n:Int
  need n > 0 else BadAmount
  need a != b else SameAccount
  need points.get a >= n else NotEnoughPoints (n - points.get a)
  ok otherwise
  keep ledger.total
  ensure points.get' a = points.get a - n
  ensure points.get' b = points.get b + n
  eg A:0@u1 B:0@u2 points.A:10 | transfer_points A B 4 -> ok | points.A:6 points.B:4
body
  points.set a (points.get a - n)
  points.set b (points.get b + n)

purchase_earning buyer:Account escrow:Account seller:Account house:Account amount:Int
  need as purchase buyer escrow seller house amount
  ok otherwise
  keep ledger.total
  ensure points.get' buyer = points.get buyer + points_for amount
  ensure ledger.balance' escrow = ledger.balance escrow
  emit as purchase buyer escrow seller house amount
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 | purchase_earning A E B H 200 -> ok | A:300 B:196 H:4 points.A:2
body
  purchase buyer escrow seller house amount
  earn_points buyer amount

subscribe id:Int buyer:Account amount:Int
  need sub_amount.get id = 0 else SubExists
  need amount > 0 else BadAmount
  need frozen.get buyer = 0 else Frozen
  ok otherwise
  keep ledger.total
  ensure sub_amount.get' id = amount
  ensure sub_due.get' id = clock.now + 30
  emit Subscribed id buyer amount clock.now
  eg A:0@u1 now:5 | subscribe 2 A 40 -> ok | sub_amount.2:40 sub_due.2:35
body
  sub_amount.set id amount
  sub_due.set id (clock.now + 30)
  audit.emit (Subscribed id buyer amount clock.now)

change_sub id:Int amount:Int
  need sub_amount.get id > 0 else NoSub
  need amount > 0 else BadAmount
  ok otherwise
  keep ledger.total
  ensure sub_amount.get' id = amount
  eg sub_amount.2:40 | change_sub 2 60 -> ok | sub_amount.2:60
body
  sub_amount.set id amount

cancel_sub id:Int
  need sub_amount.get id > 0 else NoSub
  ok otherwise
  keep ledger.total
  ensure sub_amount.get' id = 0
  ensure paused.get' id = 0
  emit SubCancelled id clock.now
  eg sub_amount.2:40 | cancel_sub 2 -> ok | sub_amount.2:0
body
  sub_amount.set id 0
  paused.set id 0
  audit.emit (SubCancelled id clock.now)

pause_sub id:Int
  need sub_amount.get id > 0 else NoSub
  need paused.get id = 0 else Paused
  ok otherwise
  keep ledger.total
  ensure paused.get' id = 1
  eg sub_amount.2:40 | pause_sub 2 -> ok | paused.2:1
body
  paused.set id 1

resume_sub id:Int
  need paused.get id = 1 else NotPaused
  ok otherwise
  keep ledger.total
  ensure paused.get' id = 0
  ensure sub_due.get' id = clock.now + 30
  eg sub_amount.2:40 paused.2:1 now:50 | resume_sub 2 -> ok | paused.2:0 sub_due.2:80
body
  paused.set id 0
  sub_due.set id (clock.now + 30)

charge_sub id:Int buyer:Account escrow:Account seller:Account house:Account
  need sub_amount.get id > 0 else NoSub
  need paused.get id = 0 else Paused
  need clock.now >= sub_due.get id else NotDue (sub_due.get id - clock.now)
  need as purchase buyer escrow seller house (sub_amount.get id)
  ok otherwise
  keep ledger.total
  ensure sub_due.get' id = sub_due.get id + 30
  ensure ledger.balance' escrow = ledger.balance escrow
  emit as purchase buyer escrow seller house (sub_amount.get id)
  emit Charged id clock.now
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 sub_amount.2:200 sub_due.2:30 now:40 | charge_sub 2 A E B H -> ok | A:300 B:196 H:4 sub_due.2:60
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 sub_amount.2:200 sub_due.2:30 now:10 | charge_sub 2 A E B H -> NotDue 20
body
  purchase buyer escrow seller house (sub_amount.get id)
  sub_due.set id (sub_due.get id + 30)
  audit.emit (Charged id clock.now)

charge_sub_with_points id:Int buyer:Account escrow:Account seller:Account house:Account
  need as charge_sub id buyer escrow seller house
  ok otherwise
  keep ledger.total
  ensure points.get' buyer = points.get buyer + points_for (sub_amount.get id)
  ensure sub_due.get' id = sub_due.get id + 30
  emit as charge_sub id buyer escrow seller house
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 sub_amount.2:200 sub_due.2:30 now:40 | charge_sub_with_points 2 A E B H -> ok | A:300 points.A:2
body
  earn_points buyer (sub_amount.get id)
  charge_sub id buyer escrow seller house

charge_late id:Int buyer:Account house:Account days:Int
  need sub_amount.get id > 0 else NoSub
  need as move buyer house (late_fee days)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' house = ledger.balance house + late_fee days
  emit as move buyer house (late_fee days)
  eg A:100@u1 H:0@u9 sub_amount.2:40 | charge_late 2 A H 3 -> ok | A:85 H:15
  eg A:100@u1 H:0@u9 sub_amount.2:40 | charge_late 2 A H 0 -> ok | A:100 H:0
body
  move buyer house (late_fee days)

chargeback id:Int seller:Account buyer:Account
  need captured.get id > 0 else NotCaptured
  need as transfer seller buyer (captured.get id)
  ok otherwise
  keep ledger.total
  ensure captured.get' id = 0
  ensure ledger.balance' buyer = ledger.balance buyer + captured.get id
  emit as transfer seller buyer (captured.get id)
  emit ChargedBack id clock.now
  eg B:300@u2 A:0@u1 captured.7:250 | chargeback 7 B A -> ok | B:50 A:250 captured.7:0
  eg B:300@u2 A:0@u1 | chargeback 7 B A -> NotCaptured
body
  transfer seller buyer (captured.get id)
  captured.set id 0
  audit.emit (ChargedBack id clock.now)

payout_holdback seller:Account reserve:Account bank:Account amount:Int
  need amount > 0 else BadAmount
  need distinct seller reserve bank else SameAccount
  need ledger.balance seller >= amount else Insufficient (amount - ledger.balance seller)
  need as withdraw seller bank (amount - holdback_for amount)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' seller = ledger.balance seller - amount
  ensure ledger.balance' reserve = ledger.balance reserve + holdback_for amount
  ensure spent.get' seller = spent.get seller + amount - holdback_for amount
  emit as move seller reserve (holdback_for amount)
  emit as withdraw seller bank (amount - holdback_for amount)
  emit Held seller (holdback_for amount) clock.now
  eg B:500@u2 R:0@u9 K:0@u9 limit.B:1000 verified.B:1 | payout_holdback B R K 200 -> ok | B:300 R:20 K:180 spent.B:180
body
  move seller reserve (holdback_for amount)
  withdraw seller bank (amount - holdback_for amount)
  audit.emit (Held seller (holdback_for amount) clock.now)

release_holdback reserve:Account seller:Account amount:Int
  need as transfer reserve seller amount
  ok otherwise
  keep ledger.total
  ensure ledger.balance' seller = ledger.balance seller + amount
  emit as transfer reserve seller amount
  emit HoldbackReleased seller amount clock.now
  eg R:20@u9 B:0@u2 | release_holdback R B 20 -> ok | R:0 B:20
body
  transfer reserve seller amount
  audit.emit (HoldbackReleased seller amount clock.now)

checkout2 id1:Int id2:Int buyer:Account escrow:Account seller1:Account seller2:Account house:Account a1:Int a2:Int
  need id1 != id2 else SameOrder
  need distinct buyer escrow seller1 house and distinct buyer escrow seller2 house else SameAccount
  need as purchase_order id1 buyer escrow seller1 house a1
  need as purchase_order id2 buyer escrow seller2 house a2
  need ledger.balance buyer >= a1 + a2 else Insufficient (a1 + a2 - ledger.balance buyer)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - a1 - a2
  ensure ledger.balance' escrow = ledger.balance escrow
  emit as purchase_order id1 buyer escrow seller1 house a1
  emit as purchase_order id2 buyer escrow seller2 house a2
  emit CheckedOut buyer (a1 + a2) clock.now
  eg A:500@u1 E:0@u0 B:0@u2 C:0@u3 H:0@u9 | checkout2 1 2 A E B C H 200 100 -> ok | A:200 B:196 C:98 H:6
  eg A:250@u1 E:0@u0 B:0@u2 C:0@u3 H:0@u9 | checkout2 1 2 A E B C H 200 100 -> Insufficient 50
body
  purchase_order id1 buyer escrow seller1 house a1
  purchase_order id2 buyer escrow seller2 house a2
  audit.emit (CheckedOut buyer (a1 + a2) clock.now)

checkout_vat id:Int buyer:Account escrow:Account seller:Account house:Account tax:Account amount:Int
  need distinct buyer escrow seller house tax else SameAccount
  need as purchase_order id buyer escrow seller house amount
  need ledger.balance buyer >= amount + vat_of amount else Insufficient (amount + vat_of amount - ledger.balance buyer)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - amount - vat_of amount
  ensure ledger.balance' tax = ledger.balance tax + vat_of amount
  emit as purchase_order id buyer escrow seller house amount
  emit as move buyer tax (vat_of amount)
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 T:0@u8 | checkout_vat 1 A E B H T 200 -> ok | A:250 B:196 H:4 T:50
body
  purchase_order id buyer escrow seller house amount
  move buyer tax (vat_of amount)

checkout_tip id:Int buyer:Account escrow:Account seller:Account house:Account amount:Int extra:Int
  need distinct buyer escrow seller house else SameAccount
  need as purchase_order id buyer escrow seller house amount
  need extra >= 0 else BadAmount
  need ledger.balance buyer >= amount + extra else Insufficient (amount + extra - ledger.balance buyer)
  ok otherwise
  keep ledger.total
  ensure ledger.balance' buyer = ledger.balance buyer - amount - extra
  emit as purchase_order id buyer escrow seller house amount
  emit as tip buyer seller extra
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 | checkout_tip 1 A E B H 200 10 -> ok | A:290 B:206 H:4
body
  purchase_order id buyer escrow seller house amount
  tip buyer seller extra

checkout_points id:Int buyer:Account escrow:Account seller:Account house:Account amount:Int
  need distinct buyer escrow seller house else SameAccount
  need as purchase_order id buyer escrow seller house amount
  ok otherwise
  keep ledger.total
  ensure points.get' buyer = points.get buyer + points_for amount
  ensure ledger.balance' buyer = ledger.balance buyer - amount
  emit as purchase_order id buyer escrow seller house amount
  eg A:500@u1 E:0@u0 B:0@u2 H:0@u9 | checkout_points 1 A E B H 200 -> ok | A:300 points.A:2
body
  purchase_order id buyer escrow seller house amount
  earn_points buyer amount

close_account a:Account
  need ledger.balance a = 0 else NotEmpty (ledger.balance a)
  ok otherwise
  keep ledger.total
  ensure frozen.get' a = 1
  ensure verified.get' a = 0
  emit Closed a clock.now
  eg A:0@u1 verified.A:1 | close_account A -> ok | frozen.A:1 verified.A:0
  eg A:5@u1 | close_account A -> NotEmpty 5
body
  frozen.set a 1
  verified.set a 0
  audit.emit (Closed a clock.now)

reopen_account a:Account
  need frozen.get a = 1 else NotFrozen
  ok otherwise
  keep ledger.total
  ensure frozen.get' a = 0
  emit as unfreeze a
  emit Reopened a clock.now
  eg A:0@u1 frozen.A:1 | reopen_account A -> ok | frozen.A:0
body
  unfreeze a
  audit.emit (Reopened a clock.now)
