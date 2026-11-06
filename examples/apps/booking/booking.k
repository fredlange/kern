const rooms 5
const days 7
const slots 35

record Booking room:Int day:Int guest:Int name:Text

state stay Int -> Booking

invariant s:Int stay.guest.get s >= 0

book b:Booking
  need b.room >= 0 else NoSuchRoom
  need b.room < rooms else NoSuchRoom
  need b.day >= 0 else NoSuchDay
  need b.day < days else NoSuchDay
  need b.guest > 0 else NoGuest
  need length b.name > 0 else NoName
  need stay.guest.get (b.room * days + b.day) = 0 else Taken
  ok otherwise
  ensure stay.get' (b.room * days + b.day) = b
  ensure stay.get' s = stay.get s when s != b.room * days + b.day for s from 0 to slots
  emit Booked b.room b.day b.guest
  eg | book 2 3 9 "ann" -> ok | stay.guest.17:9 stay.name.17:"ann"
  eg stay.guest.17:4 | book 2 3 9 "ann" -> Taken
  eg | book 5 0 9 "ann" -> NoSuchRoom
  eg | book -1 0 9 "ann" -> NoSuchRoom
  eg | book 0 -1 9 "ann" -> NoSuchDay
  eg | book 4 6 9 "ann" -> ok | stay.guest.34:9
  eg | book 0 7 9 "ann" -> NoSuchDay
  eg | book 0 0 0 "ann" -> NoGuest
  eg | book 0 0 3 "" -> NoName
  eg | book 0 0 3 "b" -> ok | stay.name.0:"b"
body
  stay.set (b.room * days + b.day) b
  audit.emit (Booked b.room b.day b.guest)

cancel room:Int day:Int guest:Int
  need room >= 0 else NoSuchRoom
  need room < rooms else NoSuchRoom
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  need stay.guest.get (room * days + day) = guest else NotYours
  need guest > 0 else NoGuest
  ok otherwise
  ensure stay.get' (room * days + day) = Booking room day 0 ""
  ensure stay.guest.get' s = stay.guest.get s when s != room * days + day for s from 0 to slots
  emit Cancelled room day guest
  eg stay.guest.17:9 stay.name.17:"ann" | cancel 2 3 9 -> ok | stay.guest.17:0 stay.name.17:""
  eg stay.guest.17:4 | cancel 2 3 9 -> NotYours
  eg stay.guest.0:9 | cancel 0 0 9 -> ok | stay.guest.0:0
  eg stay.guest.34:9 | cancel 4 6 9 -> ok | stay.guest.34:0
  eg | cancel -1 0 9 -> NoSuchRoom
  eg | cancel 5 0 9 -> NoSuchRoom
  eg | cancel 0 -1 9 -> NoSuchDay
  eg | cancel 0 7 9 -> NoSuchDay
  eg stay.guest.0:0 | cancel 0 0 0 -> NoGuest
body
  stay.set (room * days + day) (Booking room day 0 "")
  audit.emit (Cancelled room day guest)

nights_booked room:Int -> Int
  ensure result >= 0
  ensure result <= days
  eg stay.guest.14:3 stay.guest.16:5 | nights_booked 2 -> ok
body
  result = sum d from 0 to days of 1 where stay.guest.get (room * days + d) > 0

guest_nights g:Int -> Int
  ensure result >= 0
  ensure result <= slots
  eg stay.guest.3:4 stay.guest.20:4 stay.guest.21:5 | guest_nights 4 -> ok
body
  result = sum s from 0 to slots of 1 where stay.guest.get s = g

busy_on day:Int -> Int
  ensure result >= 0
  ensure result <= rooms
body
  result = sum r from 0 to rooms of 1 where stay.guest.get (r * days + day) > 0

clear_day day:Int
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  ok otherwise
  ensure stay.guest.get' (r * days + day) = 0 for r from 0 to rooms
  ensure stay.name.get' (r * days + day) = "" for r from 0 to rooms
  ensure stay.guest.get' s = stay.guest.get s when s - (s / days) * days != day for s from 0 to slots
  eg stay.guest.3:4 stay.guest.10:5 stay.guest.11:6 stay.name.3:"a" stay.name.10:"b" stay.name.11:"c" | clear_day 3 -> ok | stay.guest.3:0 stay.guest.10:0 stay.guest.11:6 stay.name.3:"" stay.name.10:"" stay.name.11:"c"
  eg stay.guest.0:4 stay.guest.7:5 stay.guest.1:6 | clear_day 0 -> ok | stay.guest.0:0 stay.guest.7:0 stay.guest.1:6
  eg | clear_day -1 -> NoSuchDay
  eg | clear_day 7 -> NoSuchDay
body
  for r from 0 to rooms
    invariant stay.guest.get' (q * days + day) = 0 for q from 0 to r
    invariant stay.name.get' (q * days + day) = "" for q from 0 to r
    invariant stay.guest.get' s = stay.guest.get s when s - (s / days) * days != day or s / days >= r for s from 0 to slots
    stay.name.set (r * days + day) ""
    stay.guest.set (r * days + day) 0

clear_all
  ok otherwise
  ensure stay.guest.get' s = 0 for s from 0 to slots
  ensure stay.name.get' s = "" for s from 0 to slots
  eg stay.guest.3:4 stay.guest.20:5 stay.name.3:"a" | clear_all -> ok | stay.guest.3:0 stay.guest.20:0 stay.name.3:""
body
  for r from 0 to rooms
    invariant stay.guest.get' s = 0 for s from 0 to (r * days)
    invariant stay.name.get' s = "" for s from 0 to (r * days)
    for d from 0 to days
      invariant stay.guest.get' s = 0 for s from 0 to (r * days)
      invariant stay.name.get' s = "" for s from 0 to (r * days)
      invariant stay.guest.get' (r * days + e) = 0 for e from 0 to d
      invariant stay.name.get' (r * days + e) = "" for e from 0 to d
      stay.guest.set (r * days + d) 0
      stay.name.set (r * days + d) ""

seat room:Int day:Int width:Int guest:Int
  need width > 0 else BadWidth
  need day >= 0 else NoSuchDay
  need day < width else NoSuchDay
  need guest > 0 else NoGuest
  ok otherwise
  ensure stay.guest.get' (room * width + day) = guest
  eg | seat 2 3 7 4 -> ok | stay.guest.17:4
  eg | seat 2 7 0 4 -> BadWidth
  eg | seat 0 0 7 4 -> ok | stay.guest.0:4
  eg | seat 0 7 7 4 -> NoSuchDay
  eg | seat 0 -1 7 4 -> NoSuchDay
  eg | seat 0 0 7 0 -> NoGuest
body
  stay.guest.set (room * width + day) guest
