import money.ledger

state tickets Int

invariant p:Int tickets.get p >= 0

buy_ticket player:Int buyer:Account pot:Account
  need as transfer buyer pot 10
  ok otherwise
  keep ledger.total
  ensure tickets.get' player = tickets.get player + 1
  emit as transfer buyer pot 10
  eg B:25@u1 P:0@u0 | buy_ticket 4 B P -> ok | B:15 P:10 tickets.4:1
  eg B:5@u1 P:0@u0 | buy_ticket 4 B P -> Insufficient 5
body
  transfer buyer pot 10
  tickets.set player (tickets.get player + 1)

loyalty_bonus lo:Int hi:Int
  ok otherwise
  ensure tickets.get' j = tickets.get j + 1 when tickets.get j >= 5 for j from lo to hi
  ensure tickets.get' j = tickets.get j when tickets.get j < 5 for j from lo to hi
  eg tickets.1:5 tickets.2:4 | loyalty_bonus 1 3 -> ok | tickets.1:6 tickets.2:4
body
  for k from lo to hi
    invariant tickets.get' j = tickets.get j + 1 when tickets.get j >= 5 for j from lo to k
    invariant tickets.get' j = tickets.get j when tickets.get j < 5 for j from lo to k
    if tickets.get k >= 5
      tickets.set k (tickets.get k + 1)
