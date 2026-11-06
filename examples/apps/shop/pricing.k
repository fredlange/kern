const max_discount 50

state unit Int -> Decimal
state buyer Int -> Account

invariant i:Int unit.get i >= 0

set_unit item:Int p:Decimal
  need item > 0 else NoSuchItem
  need p >= 0 else BadPrice
  ok otherwise
  ensure unit.get' item = p
  eg | set_unit 3 2.5 -> ok | unit.3:2.5
  eg | set_unit 0 2.5 -> NoSuchItem
  eg | set_unit 3 -1 -> BadPrice
  eg | set_unit 3 0 -> ok
body
  unit.set item p

line_total item:Int qty:Int -> Decimal
  ensure result >= 0 when qty >= 0
  ensure result = unit.get item * qty
  eg unit.3:2.5 | line_total 3 4 -> ok
body
  result = unit.get item * qty

discounted item:Int qty:Int pct:Int -> Decimal
  ensure result = unit.get item * qty * (100 - pct) / 100
  ensure result <= unit.get item * qty when pct >= 0 and qty >= 0
  eg unit.3:2.5 | discounted 3 4 10 -> ok
  eg unit.3:2.5 | discounted 3 4 0 -> ok
  eg unit.3:2.5 | discounted 3 0 10 -> ok
body
  result = unit.get item * qty * (100 - pct) / 100

claim order:Int who:Account
  need order > 0 else BadOrder
  need buyer.get order = none else AlreadyClaimed
  ok otherwise
  ensure buyer.get' order = who
  eg A:0@u1 | claim 1 A -> ok | buyer.1:A
  eg A:0@u1 | claim 0 A -> BadOrder
  eg A:0@u1 B:0@u2 buyer.1:B | claim 1 A -> AlreadyClaimed
body
  buyer.set order who

release order:Int
  need order > 0 else BadOrder
  ok otherwise
  ensure buyer.get' order = none
  eg A:0@u1 buyer.1:A | release 1 -> ok | buyer.1:none
  eg | release 0 -> BadOrder
body
  buyer.set order none
