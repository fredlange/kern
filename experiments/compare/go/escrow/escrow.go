package escrow

import (
	"errors"
	"fmt"
)

var (
	ErrBadAmount   = errors.New("bad amount")
	ErrBadFee      = errors.New("bad fee")
	ErrBadPart     = errors.New("bad part")
	ErrSameAccount = errors.New("same account")
	ErrOverdrawn   = errors.New("overdrawn")
)

// InsufficientError reports how much the account is short.
type InsufficientError struct{ Short int }

func (e *InsufficientError) Error() string { return fmt.Sprintf("insufficient funds: short %d", e.Short) }

type Moved struct {
	From, To Account
	Amount   int
	At       int64
}
type Reserved struct {
	Buyer  Account
	Amount int
	At     int64
}
type Released struct {
	Buyer  Account
	Amount int
	At     int64
}
type Captured struct {
	Seller Account
	Amount int
	At     int64
}
type Cancelled struct {
	Buyer Account
	At    int64
}
type Tipped struct {
	Seller Account
	Amount int
	At     int64
}

func (Moved) isEvent()     {}
func (Reserved) isEvent()  {}
func (Released) isEvent()  {}
func (Captured) isEvent()  {}
func (Cancelled) isEvent() {}
func (Tipped) isEvent()    {}

func (s *Service) needFunds(a Account, amount int) error {
	if bal := s.Ledger.Balance(a); bal < amount {
		return &InsufficientError{Short: amount - bal}
	}
	return nil
}

func (s *Service) Transfer(from, to Account, amount int) error {
	if amount <= 0 {
		return ErrBadAmount
	}
	if from == to {
		return ErrSameAccount
	}
	if err := s.needFunds(from, amount); err != nil {
		return err
	}
	s.Ledger.Debit(from, amount)
	s.Ledger.Credit(to, amount)
	s.Emit(Moved{from, to, amount, s.Now})
	return nil
}

// Move is like Transfer, but moving 0 is allowed and does nothing.
func (s *Service) Move(from, to Account, amount int) error {
	if amount < 0 {
		return ErrBadAmount
	}
	if amount == 0 {
		return nil
	}
	return s.Transfer(from, to, amount)
}

func (s *Service) Reserve(buyer, escrow Account, amount int) error {
	if err := s.Transfer(buyer, escrow, amount); err != nil {
		return err
	}
	s.Emit(Reserved{buyer, amount, s.Now})
	return nil
}

func (s *Service) Release(escrow, buyer Account, amount int) error {
	if err := s.Transfer(escrow, buyer, amount); err != nil {
		return err
	}
	s.Emit(Released{buyer, amount, s.Now})
	return nil
}

// Capture pays the seller from escrow. From 100 up, the house takes fee.
func (s *Service) Capture(escrow, seller, house Account, amount, fee int) error {
	if amount <= 0 {
		return ErrBadAmount
	}
	if fee < 0 || fee > amount {
		return ErrBadFee
	}
	if escrow == seller || escrow == house || seller == house {
		return ErrSameAccount
	}
	if err := s.needFunds(escrow, amount); err != nil {
		return err
	}
	if amount < 100 {
		if err := s.Transfer(escrow, seller, amount); err != nil {
			return err
		}
	} else {
		if err := s.Move(escrow, seller, amount-fee); err != nil {
			return err
		}
		if err := s.Move(escrow, house, fee); err != nil {
			return err
		}
	}
	s.Emit(Captured{seller, amount, s.Now})
	return nil
}

func (s *Service) Purchase(buyer, escrow, seller, house Account, amount, fee int) error {
	if amount <= 0 {
		return ErrBadAmount
	}
	if fee < 0 || fee > amount {
		return ErrBadFee
	}
	if buyer == escrow || escrow == seller || escrow == house || seller == house {
		return ErrSameAccount
	}
	if err := s.needFunds(buyer, amount); err != nil {
		return err
	}
	if s.Ledger.Balance(escrow) < 0 {
		return ErrOverdrawn
	}
	if err := s.Reserve(buyer, escrow, amount); err != nil {
		return err
	}
	return s.Capture(escrow, seller, house, amount, fee)
}

func (s *Service) Cancel(escrow, buyer Account, amount int) error {
	if err := s.Release(escrow, buyer, amount); err != nil {
		return err
	}
	s.Emit(Cancelled{buyer, s.Now})
	return nil
}

// Refund gives part back to the buyer; the rest is captured for the seller.
func (s *Service) Refund(escrow, buyer, seller, house Account, amount, part, fee int) error {
	if part <= 0 || part > amount {
		return ErrBadPart
	}
	if part != amount && (fee < 0 || fee > amount-part) {
		return ErrBadFee
	}
	if escrow == buyer || escrow == seller || escrow == house || seller == house {
		return ErrSameAccount
	}
	if err := s.needFunds(escrow, amount); err != nil {
		return err
	}
	if part == amount {
		return s.Release(escrow, buyer, amount)
	}
	if err := s.Move(escrow, buyer, part); err != nil {
		return err
	}
	return s.Capture(escrow, seller, house, amount-part, fee)
}

func (s *Service) Split(escrow, a, b Account, amount, part int) error {
	if part < 0 || part > amount {
		return ErrBadPart
	}
	if escrow == a || escrow == b || a == b {
		return ErrSameAccount
	}
	if err := s.needFunds(escrow, amount); err != nil {
		return err
	}
	if err := s.Move(escrow, a, part); err != nil {
		return err
	}
	return s.Move(escrow, b, amount-part)
}

func (s *Service) Tip(buyer, seller Account, amount int) error {
	if err := s.Move(buyer, seller, amount); err != nil {
		return err
	}
	s.Emit(Tipped{seller, amount, s.Now})
	return nil
}
