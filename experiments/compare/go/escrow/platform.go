// Not counted: the equivalent of Kern's handlers (ledger, clock, audit) and atomic run().
package escrow

import "maps"

type Account string

type Ledger struct{ Balances map[Account]int }

func (l *Ledger) Balance(a Account) int {
	b, ok := l.Balances[a]
	if !ok {
		panic("unknown account " + string(a))
	}
	return b
}
func (l *Ledger) Debit(a Account, n int)  { l.Balances[a] = l.Balance(a) - n }
func (l *Ledger) Credit(a Account, n int) { l.Balances[a] = l.Balance(a) + n }

type Event interface{ isEvent() }

type Service struct {
	Ledger *Ledger
	Events []Event
	Now    int64
}

func (s *Service) Emit(e Event) { s.Events = append(s.Events, e) }

// Atomic runs fn; if it returns an error, balances and events are restored.
func (s *Service) Atomic(fn func() error) error {
	bal, n := maps.Clone(s.Ledger.Balances), len(s.Events)
	if err := fn(); err != nil {
		s.Ledger.Balances, s.Events = bal, s.Events[:n]
		return err
	}
	return nil
}
