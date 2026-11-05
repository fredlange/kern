channel net

state pending Int
state want Int
state deadline Int
state next Int
state source Int
state ref Int
state votes Int
state last Int
state body Int -> Text

invariant s:Int pending.get s = 0 or pending.get s = 1
invariant i:Int votes.get i >= 0
invariant i:Int ref.get i >= 0

request src:Int
  need src = 1 or src = 2 else UnknownSource
  need pending.get src = 0 else AlreadyPending
  need clock.now >= last.get src + 5 else TooSoon (last.get src + 5 - clock.now)
  ok otherwise
  ensure last.get' src = clock.now
  ensure next.get' 0 = next.get 0 + 1
  ensure pending.get' src = 1
  ensure want.get' src = next.get 0 + 1
  ensure deadline.get' src = clock.now + 30
  emit net FetchJoke src (next.get 0 + 1) when src = 1
  emit net FetchPun src (next.get 0 + 1) when src = 2
  eg now:100 | request 1 -> ok | next.0:1 pending.1:1 want.1:1 last.1:100
  eg now:100 next.0:4 | request 2 -> ok | next.0:5 pending.2:1 want.2:5
  eg pending.1:1 | request 1 -> AlreadyPending
  eg | request 3 -> UnknownSource
  eg now:12 last.1:10 | request 1 -> TooSoon 3
  eg now:15 last.1:10 | request 1 -> ok | last.1:15
body
  slot = next.get 0 + 1
  next.set 0 slot
  pending.set src 1
  want.set src slot
  deadline.set src (clock.now + 30)
  last.set src clock.now
  if src = 1
    net.emit (FetchJoke src slot)
  else
    net.emit (FetchPun src slot)

store src:Int slot:Int id:Int text:Text
  need src = 1 or src = 2 else UnknownSource
  need pending.get src = 1 else Stale
  need want.get src = slot else Stale
  need id > 0 else BadId
  need length text > 0 else Empty
  need length text <= 500 else TooLong
  ok otherwise
  keep next.get 0
  ensure source.get' slot = src
  ensure ref.get' slot = id
  ensure body.get' slot = text
  ensure pending.get' src = 0
  emit Stored slot src id clock.now
  eg pending.1:1 want.1:3 | store 1 3 44 "a | b" -> ok | source.3:1 ref.3:44 pending.1:0 body.3:"a | b"
  eg pending.1:1 want.1:3 | store 1 2 44 "x" -> Stale
  eg want.1:3 | store 1 3 44 "x" -> Stale
  eg pending.1:1 want.1:3 | store 1 3 0 "x" -> BadId
  eg pending.1:1 want.1:3 | store 1 3 44 "" -> Empty
body
  source.set slot src
  ref.set slot id
  body.set slot text
  pending.set src 0
  audit.emit (Stored slot src id clock.now)

store_joke src:Int slot:Int id:Int setup:Text punchline:Text
  need as store src slot id (setup + " " + punchline)
  ok otherwise
  ensure body.get' slot = setup + " " + punchline
  emit as store src slot id (setup + " " + punchline)
  eg pending.1:1 want.1:3 | store_joke 1 3 44 "Why?" "Because." -> ok | body.3:"Why? Because."
body
  store src slot id (setup + " " + punchline)

store_pun src:Int slot:Int id:Int joke:Text
  need as store src slot id joke
  ok otherwise
  ensure body.get' slot = joke
  emit as store src slot id joke
  eg pending.2:1 want.2:5 | store_pun 2 5 7 "You have my Word!" -> ok | body.5:"You have my Word!"
body
  store src slot id joke

fetch_failed src:Int slot:Int
  need src = 1 or src = 2 else UnknownSource
  need pending.get src = 1 else Stale
  need want.get src = slot else Stale
  ok otherwise
  keep next.get 0
  ensure pending.get' src = 0
  ensure ref.get' slot = ref.get slot
  emit FetchFailed src slot clock.now
  eg pending.1:1 want.1:3 | fetch_failed 1 3 -> ok | pending.1:0
  eg pending.1:1 want.1:4 | fetch_failed 1 3 -> Stale
body
  pending.set src 0
  audit.emit (FetchFailed src slot clock.now)

give_up src:Int
  need src = 1 or src = 2 else UnknownSource
  need pending.get src = 1 else NotPending
  need clock.now >= deadline.get src else NotYet (deadline.get src - clock.now)
  ok otherwise
  keep next.get 0
  ensure pending.get' src = 0
  emit GaveUp src (want.get src) clock.now
  eg now:40 pending.1:1 want.1:3 deadline.1:30 | give_up 1 -> ok | pending.1:0
  eg now:10 pending.1:1 deadline.1:30 | give_up 1 -> NotYet 20
body
  pending.set src 0
  audit.emit (GaveUp src (want.get src) clock.now)

upvote item:Int
  need ref.get item > 0 else NoSuchItem
  ok otherwise
  keep next.get 0
  keep ref.get item
  ensure votes.get' item = votes.get item + 1
  emit Voted item (votes.get item + 1) clock.now
  eg ref.2:44 | upvote 2 -> ok | votes.2:1
  eg ref.2:44 | upvote 3 -> NoSuchItem
body
  n = votes.get item + 1
  votes.set item n
  audit.emit (Voted item n clock.now)
