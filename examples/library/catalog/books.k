state copies Int

invariant b:Int copies.get b >= 0

add_copies book:Int n:Int
  need n > 0 else BadCount
  ok otherwise
  ensure copies.get' book = copies.get book + n
  eg copies.3:1 | add_copies 3 2 -> ok | copies.3:3
body
  copies.set book (copies.get book + n)
