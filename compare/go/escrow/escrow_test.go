package escrow

import (
	"errors"
	"testing"
)

type B = map[Account]int

func check(t *testing.T, bal B, call func(s *Service) error, want error, after B) {
	t.Helper()
	s := &Service{Ledger: &Ledger{Balances: bal}}
	err := s.Atomic(func() error { return call(s) })
	var ins, wantIns *InsufficientError
	switch {
	case errors.As(want, &wantIns):
		if !errors.As(err, &ins) || ins.Short != wantIns.Short {
			t.Fatalf("got %v, want %v", err, want)
		}
	case !errors.Is(err, want):
		t.Fatalf("got %v, want %v", err, want)
	}
	for a, b := range after {
		if got := s.Ledger.Balance(a); got != b {
			t.Fatalf("%s: got %d, want %d", a, got, b)
		}
	}
}

func TestTransfer(t *testing.T) {
	check(t, B{"A": 500, "B": 0}, func(s *Service) error { return s.Transfer("A", "B", 100) }, nil, B{"A": 400, "B": 100})
	check(t, B{"A": 50, "B": 0}, func(s *Service) error { return s.Transfer("A", "B", 100) }, &InsufficientError{50}, nil)
	check(t, B{"A": 500, "B": 0}, func(s *Service) error { return s.Transfer("A", "A", 100) }, ErrSameAccount, nil)
	check(t, B{"A": 500, "B": 0}, func(s *Service) error { return s.Transfer("A", "B", 0) }, ErrBadAmount, nil)
}

func TestMove(t *testing.T) {
	check(t, B{"A": 500, "B": 0}, func(s *Service) error { return s.Move("A", "B", 0) }, nil, B{"A": 500, "B": 0})
	check(t, B{"A": 500, "B": 0}, func(s *Service) error { return s.Move("A", "B", 100) }, nil, B{"A": 400, "B": 100})
	check(t, B{"A": 0}, func(s *Service) error { return s.Move("A", "A", 0) }, nil, B{"A": 0})
}

func TestReserveRelease(t *testing.T) {
	check(t, B{"A": 500, "E": 0}, func(s *Service) error { return s.Reserve("A", "E", 100) }, nil, B{"A": 400, "E": 100})
	check(t, B{"A": 50, "E": 0}, func(s *Service) error { return s.Reserve("A", "E", 100) }, &InsufficientError{50}, nil)
	check(t, B{"A": 400, "E": 100}, func(s *Service) error { return s.Release("E", "A", 100) }, nil, B{"A": 500, "E": 0})
}

func TestCapture(t *testing.T) {
	check(t, B{"E": 100, "B": 0, "H": 0}, func(s *Service) error { return s.Capture("E", "B", "H", 50, 5) }, nil, B{"E": 50, "B": 50, "H": 0})
	check(t, B{"E": 500, "B": 0, "H": 0}, func(s *Service) error { return s.Capture("E", "B", "H", 200, 10) }, nil, B{"E": 300, "B": 190, "H": 10})
	check(t, B{"E": 500, "B": 0, "H": 0}, func(s *Service) error { return s.Capture("E", "B", "H", 200, 300) }, ErrBadFee, nil)
}

func TestPurchaseCancel(t *testing.T) {
	check(t, B{"A": 500, "E": 0, "B": 0, "H": 0}, func(s *Service) error { return s.Purchase("A", "E", "B", "H", 200, 10) }, nil,
		B{"A": 300, "E": 0, "B": 190, "H": 10})
	check(t, B{"A": 400, "E": 100}, func(s *Service) error { return s.Cancel("E", "A", 100) }, nil, B{"A": 500, "E": 0})
}

func TestRefund(t *testing.T) {
	check(t, B{"E": 100, "A": 0, "B": 0, "H": 0}, func(s *Service) error { return s.Refund("E", "A", "B", "H", 100, 100, 0) }, nil,
		B{"E": 0, "A": 100, "B": 0, "H": 0})
	check(t, B{"E": 300, "A": 0, "B": 0, "H": 0}, func(s *Service) error { return s.Refund("E", "A", "B", "H", 300, 100, 10) }, nil,
		B{"E": 0, "A": 100, "B": 190, "H": 10})
}

func TestSplitTip(t *testing.T) {
	check(t, B{"E": 100, "A": 0, "B": 0}, func(s *Service) error { return s.Split("E", "A", "B", 100, 30) }, nil, B{"E": 0, "A": 30, "B": 70})
	check(t, B{"E": 100, "A": 0, "B": 0}, func(s *Service) error { return s.Split("E", "A", "B", 100, 100) }, nil, B{"E": 0, "A": 100, "B": 0})
	check(t, B{"A": 10, "B": 0}, func(s *Service) error { return s.Tip("A", "B", 0) }, nil, B{"A": 10, "B": 0})
	check(t, B{"A": 10, "B": 0}, func(s *Service) error { return s.Tip("A", "B", 5) }, nil, B{"A": 5, "B": 5})
}
