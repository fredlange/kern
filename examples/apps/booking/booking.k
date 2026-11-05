const rooms 5
const days 7
const slots 35

state slot Int
state holder Int -> Text

record Booking room:Int day:Int guest:Int name:Text


book b:Booking
  need b.room >= 0 else NoSuchRoom
  need b.room < rooms else NoSuchRoom
  need b.day >= 0 else NoSuchDay
  need b.day < days else NoSuchDay
  need b.guest > 0 else NoGuest
  need length b.name > 0 else NoName
  need slot.get (b.room * days + b.day) = 0 else Taken
  ok otherwise
  ensure slot.get' (b.room * days + b.day) = b.guest
  ensure holder.get' (b.room * days + b.day) = b.name
  ensure slot.get' s = slot.get s when s != b.room * days + b.day for s from 0 to slots
  emit Booked b.room b.day b.guest
  eg | book 2 3 9 "ann" -> ok | slot.17:9 holder.17:"ann"
  eg slot.17:4 | book 2 3 9 "ann" -> Taken
  eg | book 5 0 9 "ann" -> NoSuchRoom
  eg | book -1 0 9 "ann" -> NoSuchRoom
  eg | book 0 -1 9 "ann" -> NoSuchDay
  eg | book 4 6 9 "ann" -> ok | slot.34:9
  eg | book 0 7 9 "ann" -> NoSuchDay
  eg | book 0 0 0 "ann" -> NoGuest
  eg | book 0 0 3 "" -> NoName
  eg | book 0 0 3 "b" -> ok | holder.0:"b"
body
  slot.set (b.room * days + b.day) b.guest
  holder.set (b.room * days + b.day) b.name
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
  ensure holder.get' (room * days + day) = ""
  ensure slot.get' s = slot.get s when s != room * days + day for s from 0 to slots
  emit Cancelled room day guest
  eg slot.17:9 holder.17:"ann" | cancel 2 3 9 -> ok | slot.17:0 holder.17:""
  eg slot.17:4 | cancel 2 3 9 -> NotYours
  eg slot.0:9 | cancel 0 0 9 -> ok | slot.0:0
  eg slot.34:9 | cancel 4 6 9 -> ok | slot.34:0
  eg | cancel -1 0 9 -> NoSuchRoom
  eg | cancel 5 0 9 -> NoSuchRoom
  eg | cancel 0 -1 9 -> NoSuchDay
  eg | cancel 0 7 9 -> NoSuchDay
  eg slot.0:0 | cancel 0 0 0 -> NoGuest
body
  slot.set (room * days + day) 0
  holder.set (room * days + day) ""
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
  eg slot.0:4 slot.7:5 slot.1:6 | clear_day 0 -> ok | slot.0:0 slot.7:0 slot.1:6
  eg | clear_day -1 -> NoSuchDay
  eg | clear_day 7 -> NoSuchDay
body
  for r from 0 to rooms
    invariant slot.get' (q * days + day) = 0 for q from 0 to r
    invariant slot.get' s = slot.get s when s - (s / days) * days != day or s / days >= r for s from 0 to slots
    slot.set (r * days + day) 0

forget_names day:Int
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  ok otherwise
  ensure holder.get' (r * days + day) = "" for r from 0 to rooms
  eg holder.3:"a" holder.10:"b" holder.11:"c" | forget_names 3 -> ok | holder.3:"" holder.10:"" holder.11:"c"
  eg holder.0:"a" holder.31:"z" holder.1:"k" | forget_names 0 -> ok | holder.0:"" holder.31:"z" holder.1:"k"
  eg holder.3:"a" holder.31:"z" | forget_names 3 -> ok | holder.3:"" holder.31:""
  eg | forget_names -1 -> NoSuchDay
  eg | forget_names 7 -> NoSuchDay
body
  for r from 0 to rooms
    invariant holder.get' (q * days + day) = "" for q from 0 to r
    holder.set (r * days + day) ""

clear_all
  ok otherwise
  ensure slot.get' s = 0 for s from 0 to slots
  ensure holder.get' s = "" for s from 0 to slots
  eg slot.3:4 slot.20:5 holder.3:"a" | clear_all -> ok | slot.3:0 slot.20:0 holder.3:""
body
  for r from 0 to rooms
    invariant slot.get' s = 0 for s from 0 to (r * days)
    invariant holder.get' s = "" for s from 0 to (r * days)
    for d from 0 to days
      invariant slot.get' s = 0 for s from 0 to (r * days)
      invariant holder.get' s = "" for s from 0 to (r * days)
      invariant slot.get' (r * days + e) = 0 for e from 0 to d
      invariant holder.get' (r * days + e) = "" for e from 0 to d
      slot.set (r * days + d) 0
      holder.set (r * days + d) ""

seat room:Int day:Int width:Int guest:Int
  need width > 0 else BadWidth
  need day >= 0 else NoSuchDay
  need day < width else NoSuchDay
  need guest > 0 else NoGuest
  ok otherwise
  ensure slot.get' (room * width + day) = guest
  eg | seat 2 3 7 4 -> ok | slot.17:4
  eg | seat 2 7 0 4 -> BadWidth
  eg | seat 0 0 7 4 -> ok | slot.0:4
  eg | seat 0 7 7 4 -> NoSuchDay
  eg | seat 0 -1 7 4 -> NoSuchDay
  eg | seat 0 0 7 0 -> NoGuest
body
  slot.set (room * width + day) guest
