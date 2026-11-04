import catalog.books

state due Int
state lent Int

invariant l:Int due.get l >= 0

borrow loan:Int book:Int
  need due.get loan = 0 else LoanExists
  need copies.get book > 0 else NoCopy
  need clock.now >= 0 else BadClock
  ok otherwise
  ensure copies.get' book = copies.get book - 1
  ensure due.get' loan = clock.now + 14
  ensure lent.get' loan = book
  emit Borrowed loan book clock.now
  eg copies.3:1 now:100 | borrow 9 3 -> ok | copies.3:0 due.9:114 lent.9:3
  eg copies.3:0 now:100 | borrow 9 3 -> NoCopy
body
  copies.set book (copies.get book - 1)
  due.set loan (clock.now + 14)
  lent.set loan book
  audit.emit (Borrowed loan book clock.now)

give_back loan:Int book:Int
  need due.get loan > 0 else NoLoan
  need lent.get loan = book else WrongBook
  ok otherwise
  ensure copies.get' book = copies.get book + 1
  ensure due.get' loan = 0
  emit Returned loan book clock.now
  eg copies.3:0 due.9:114 lent.9:3 now:120 | give_back 9 3 -> ok | copies.3:1 due.9:0
  eg copies.3:0 due.9:114 lent.9:3 | give_back 9 4 -> WrongBook
body
  copies.set book (copies.get book + 1)
  due.set loan 0
  audit.emit (Returned loan book clock.now)
