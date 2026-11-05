const rooms 5
const days 7
const slots 35

state slot Int

record Booking room:Int day:Int guest:Int

invariant s:Int slot.get s >= 0

book b:Booking
  need b.room >= 0 else NoSuchRoom
  need b.room < rooms else NoSuchRoom
  need b.day >= 0 else NoSuchDay
  need b.day < days else NoSuchDay
  need b.guest > 0 else NoGuest
  need slot.get (b.room * days + b.day) = 0 else Taken
  ok otherwise
  ensure slot.get' (b.room * days + b.day) = b.guest
  ensure slot.get' s = slot.get s when s != b.room * days + b.day for s from 0 to slots
  emit Booked b.room b.day b.guest
  eg | book 2 3 9 -> ok | slot.17:9
  eg slot.17:4 | book 2 3 9 -> Taken
  eg | book 5 0 9 -> NoSuchRoom
  eg | book -1 0 9 -> NoSuchRoom
  eg | book 0 -1 9 -> NoSuchDay
  eg | book 4 6 9 -> ok | slot.34:9
  eg | book 0 7 9 -> NoSuchDay
  eg | book 0 0 0 -> NoGuest
body
  slot.set (b.room * days + b.day) b.guest
  audit.emit (Booked b.room b.day b.guest)

cancel room:Int day:Int guest:Int
  need room >= 0 else NoSuchRoom
  need room < rooms else NoSuchRoom
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  need slot.get (room * days + day) = guest else NotYours
  need guest > 0 else NoGuest
  ok otherwise
  ensure slot.get' (room * days + day) = 0
  ensure slot.get' s = slot.get s when s != room * days + day for s from 0 to slots
  emit Cancelled room day guest
  eg slot.17:9 | cancel 2 3 9 -> ok | slot.17:0
  eg slot.17:4 | cancel 2 3 9 -> NotYours
body
  slot.set (room * days + day) 0
  audit.emit (Cancelled room day guest)

nights_booked room:Int -> Int
  ensure result >= 0
  ensure result <= days
  eg slot.14:3 slot.16:5 | nights_booked 2 -> ok
body
  result = sum d from 0 to days of 1 where slot.get (room * days + d) > 0

guest_nights g:Int -> Int
  ensure result >= 0
  ensure result <= slots
  eg slot.3:4 slot.20:4 slot.21:5 | guest_nights 4 -> ok
body
  result = sum s from 0 to slots of 1 where slot.get s = g

busy_on day:Int -> Int
  ensure result >= 0
  ensure result <= rooms
body
  result = sum r from 0 to rooms of 1 where slot.get (r * days + day) > 0

clear_day day:Int
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  ok otherwise
  ensure slot.get' (r * days + day) = 0 for r from 0 to rooms
  ensure slot.get' s = slot.get s when s - (s / days) * days != day for s from 0 to slots
  eg slot.3:4 slot.10:5 slot.11:6 | clear_day 3 -> ok | slot.3:0 slot.10:0 slot.11:6
body
  for r from 0 to rooms
    invariant slot.get' (q * days + day) = 0 for q from 0 to r
    invariant slot.get' s = slot.get s when s - (s / days) * days != day or s / days >= r for s from 0 to slots
    slot.set (r * days + day) 0
