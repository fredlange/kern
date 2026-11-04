import loans
import money.ledger

state fine Int

invariant l:Int fine.get l >= 0

assess lo:Int hi:Int
  ok otherwise
  ensure fine.get' j = fine.get j + 5 when due.get j > 0 and due.get j < clock.now for j from lo to hi
  ensure fine.get' j = fine.get j when due.get j = 0 or due.get j >= clock.now for j from lo to hi
  eg due.1:50 due.2:500 fine.1:0 now:100 | assess 1 3 -> ok | fine.1:5 fine.2:0
body
  for k from lo to hi
    invariant fine.get' j = fine.get j + 5 when due.get j > 0 and due.get j < clock.now for j from lo to k
    invariant fine.get' j = fine.get j when due.get j = 0 or due.get j >= clock.now for j from lo to k
    if due.get k > 0 and due.get k < clock.now
      fine.set k (fine.get k + 5)

pay_fine loan:Int member:Account library:Account
  need fine.get loan > 0 else NoFine
  need as transfer member library (fine.get loan)
  ok otherwise
  keep ledger.total
  ensure fine.get' loan = 0
  ensure ledger.balance' member = ledger.balance member - fine.get loan
  emit as transfer member library (fine.get loan)
  eg M:20@u1 L:0@u9 fine.4:15 | pay_fine 4 M L -> ok | M:5 L:15 fine.4:0
  eg M:20@u1 L:0@u9 | pay_fine 4 M L -> NoFine
body
  transfer member library (fine.get loan)
  fine.set loan 0
