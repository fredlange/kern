state status Int
state due Int
state priority Int
state title Int -> Text

invariant t:Int status.get t >= 0
invariant t:Int status.get t <= 2
invariant t:Int priority.get t >= 0
invariant t:Int priority.get t <= 3

add_task id:Int name:Text deadline:Int prio:Int
  need id > 0 else BadId
  need status.get id = 0 else TaskExists
  need deadline >= clock.now else DeadlinePassed
  need length name > 0 else EmptyTitle
  need length name <= 40 else TitleTooLong
  need prio >= 1 else BadPriority
  need prio <= 3 else BadPriority
  ok otherwise
  ensure status.get' id = 1
  ensure due.get' id = deadline
  ensure priority.get' id = prio
  ensure title.get' id = name
  emit Added id name deadline prio
  eg | add_task 1 "write report" 10 2 -> ok | status.1:1 due.1:10 priority.1:2 title.1:"write report"
  eg status.1:1 | add_task 1 "write report" 10 2 -> TaskExists
  eg now:20 | add_task 1 "write report" 10 2 -> DeadlinePassed
  eg now:10 | add_task 1 "write report" 10 2 -> ok | status.1:1 due.1:10 priority.1:2
  eg | add_task 1 "" 10 2 -> EmptyTitle
  eg | add_task 1 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 10 2 -> TitleTooLong
  eg | add_task 1 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" 10 2 -> ok
  eg | add_task 0 "write report" 10 2 -> BadId
  eg | add_task 1 "write report" 10 0 -> BadPriority
  eg | add_task 1 "write report" 10 4 -> BadPriority
  eg | add_task 1 "write report" 10 1 -> ok | priority.1:1
  eg | add_task 1 "write report" 10 3 -> ok | priority.1:3
body
  status.set id 1
  due.set id deadline
  priority.set id prio
  title.set id name
  audit.emit (Added id name deadline prio)

complete id:Int
  need status.get id = 1 else NotOpen
  ok otherwise
  ensure status.get' id = 2
  ensure due.get' id = due.get id
  emit Completed id (clock.now <= due.get id)
  eg status.1:1 due.1:10 now:5 | complete 1 -> ok | status.1:2
  eg status.1:2 | complete 1 -> NotOpen
body
  status.set id 2
  audit.emit (Completed id (clock.now <= due.get id))

reschedule id:Int deadline:Int
  need status.get id = 1 else NotOpen
  need deadline >= clock.now else DeadlinePassed
  ok otherwise
  ensure due.get' id = deadline
  ensure status.get' id = 1
  eg status.1:1 due.1:3 now:5 | reschedule 1 9 -> ok | due.1:9
  eg status.1:1 due.1:3 now:5 | reschedule 1 4 -> DeadlinePassed
  eg status.1:1 due.1:3 now:5 | reschedule 1 5 -> ok | due.1:5
body
  due.set id deadline

retitle id:Int name:Text
  need status.get id = 1 else NotOpen
  need length name > 0 else EmptyTitle
  ok otherwise
  ensure title.get' id = name
  ensure status.get' id = 1
  eg status.1:1 title.1:"a" | retitle 1 "b" -> ok | title.1:"b"
  eg status.2:1 | retitle 1 "b" -> NotOpen
  eg status.1:1 | retitle 1 "" -> EmptyTitle
body
  title.set id name

mark_urgent id:Int
  need status.get id = 1 else NotOpen
  ok otherwise
  ensure title.get' id = "!" + (title.get id)
  ensure length (title.get' id) = length (title.get id) + 1
  eg status.1:1 title.1:"pay rent" | mark_urgent 1 -> ok | title.1:"!pay rent"
  eg status.1:2 | mark_urgent 1 -> NotOpen
body
  title.set id ("!" + title.get id)
