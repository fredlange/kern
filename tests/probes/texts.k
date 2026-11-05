state title Int -> Text
state nick Account -> Text
state size Int
state same Int

record Item name:Text qty:Int

rename k:Int t:Text
  need length t > 0 else Empty
  need t != "admin" else Reserved
  ok otherwise
  ensure title.get' k = t
  ensure size.get' k = length t
  emit Renamed k t
  eg title.1:"old" | rename 1 "new name" -> ok | title.1:"new name" size.1:8
  eg | rename 1 "" -> Empty
body
  need length t > 0 else Empty
  need t != "admin" else Reserved
  title.set k t
  size.set k (length t)
  audit.emit (Renamed k t)

append k:Int t:Text
  ok otherwise
  ensure title.get' k = title.get k + t
  ensure size.get' k = length (title.get k) + length t
body
  title.set k (title.get k + t)
  size.set k (length (title.get k))

compare j:Int k:Int
  ok otherwise
  ensure same.get' j = 1 when title.get j = title.get k
  ensure same.get' j = 0 when title.get j != title.get k
body
  if title.get j = title.get k
    same.set j 1
  else
    same.set j 0

tag a:Account t:Text
  ok otherwise
  ensure nick.get' a = t + "!"
body
  nick.set a (t + "!")

add i:Item k:Int
  need i.qty > 0 else Bad
  ok otherwise
  ensure title.get' k = i.name + " x"
body
  title.set k (i.name + " x")
  size.set k (length i.name + i.qty)
