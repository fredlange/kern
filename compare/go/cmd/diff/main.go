// Not counted: differential harness. Reads JSON-lines cases on stdin, prints results.
package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"os"

	e "compare/escrow"
)

type kase struct {
	Def   string         `json:"def"`
	Args  []any          `json:"args"`
	World map[string]int `json:"world"`
	Now   int64          `json:"now"`
}

func acc(x any) e.Account { return e.Account(x.(string)) }
func num(x any) int       { return int(x.(float64)) }

func call(s *e.Service, k kase) error {
	a := k.Args
	switch k.Def {
	case "transfer":
		return s.Transfer(acc(a[0]), acc(a[1]), num(a[2]))
	case "move":
		return s.Move(acc(a[0]), acc(a[1]), num(a[2]))
	case "reserve":
		return s.Reserve(acc(a[0]), acc(a[1]), num(a[2]))
	case "release":
		return s.Release(acc(a[0]), acc(a[1]), num(a[2]))
	case "capture":
		return s.Capture(acc(a[0]), acc(a[1]), acc(a[2]), num(a[3]), num(a[4]))
	case "purchase":
		return s.Purchase(acc(a[0]), acc(a[1]), acc(a[2]), acc(a[3]), num(a[4]), num(a[5]))
	case "cancel":
		return s.Cancel(acc(a[0]), acc(a[1]), num(a[2]))
	case "refund":
		return s.Refund(acc(a[0]), acc(a[1]), acc(a[2]), acc(a[3]), num(a[4]), num(a[5]), num(a[6]))
	case "split":
		return s.Split(acc(a[0]), acc(a[1]), acc(a[2]), num(a[3]), num(a[4]))
	case "tip":
		return s.Tip(acc(a[0]), acc(a[1]), num(a[2]))
	}
	panic(k.Def)
}

var names = map[error]string{e.ErrBadAmount: "BadAmount", e.ErrBadFee: "BadFee", e.ErrBadPart: "BadPart",
	e.ErrSameAccount: "SameAccount", e.ErrOverdrawn: "Overdrawn"}

func tuple(ev e.Event) []any {
	switch v := ev.(type) {
	case e.Moved:
		return []any{"Moved", v.From, v.To, v.Amount, v.At}
	case e.Reserved:
		return []any{"Reserved", v.Buyer, v.Amount, v.At}
	case e.Released:
		return []any{"Released", v.Buyer, v.Amount, v.At}
	case e.Captured:
		return []any{"Captured", v.Seller, v.Amount, v.At}
	case e.Cancelled:
		return []any{"Cancelled", v.Buyer, v.At}
	case e.Tipped:
		return []any{"Tipped", v.Seller, v.Amount, v.At}
	}
	panic(fmt.Sprint(ev))
}

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		var k kase
		if err := json.Unmarshal(sc.Bytes(), &k); err != nil {
			panic(err)
		}
		bal := map[e.Account]int{}
		for a, b := range k.World {
			bal[e.Account(a)] = b
		}
		s := &e.Service{Ledger: &e.Ledger{Balances: bal}, Now: k.Now}
		err := s.Atomic(func() error { return call(s, k) })
		var outcome []any
		var ins *e.InsufficientError
		switch {
		case err == nil:
			outcome = []any{"ok"}
		case errors.As(err, &ins):
			outcome = []any{"Insufficient", []int{ins.Short}}
		default:
			outcome = []any{names[err], []int{}}
		}
		events := [][]any{}
		for _, ev := range s.Events {
			events = append(events, tuple(ev))
		}
		out, _ := json.Marshal(map[string]any{"outcome": outcome, "bal": s.Ledger.Balances, "events": events})
		fmt.Println(string(out))
	}
}
