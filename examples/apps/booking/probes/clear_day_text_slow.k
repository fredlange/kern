const rooms 5
const days 7
const slots 35

state slot Int
state holder Int -> Text

record Booking room:Int day:Int guest:Int name:Text


clear_day day:Int
  need day >= 0 else NoSuchDay
  need day < days else NoSuchDay
  ok otherwise
  ensure slot.get' (r * days + day) = 0 for r from 0 to rooms
  ensure holder.get' (r * days + day) = "" for r from 0 to rooms
  ensure slot.get' s = slot.get s when s - (s / days) * days != day for s from 0 to slots
  eg slot.3:4 slot.10:5 slot.11:6 holder.3:"a" holder.10:"b" holder.11:"c" | clear_day 3 -> ok | slot.3:0 slot.10:0 slot.11:6 holder.3:"" holder.10:"" holder.11:"c"
  eg slot.0:4 slot.7:5 slot.1:6 | clear_day 0 -> ok | slot.0:0 slot.7:0 slot.1:6
  eg | clear_day -1 -> NoSuchDay
  eg | clear_day 7 -> NoSuchDay
body
  for r from 0 to rooms
    invariant slot.get' (q * days + day) = 0 for q from 0 to r
    invariant holder.get' (q * days + day) = "" for q from 0 to r
    invariant slot.get' s = slot.get s when s - (s / days) * days != day or s / days >= r for s from 0 to slots
    holder.set (r * days + day) ""
    slot.set (r * days + day) 0
