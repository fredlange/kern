// Go in the style a Kern->Go compiler would emit: checked int64, undo log, event buffer.
package main
import ("fmt"; "math"; "os"; "strconv"; "sync"; "syscall"; "time")
const N = 1_000_000
const (Ok = iota; Bad; Same; Insuf; Ovf)
type ev struct{ c uint8; f, t uint32; a, now int64 }
type undo struct{ k int; v int64 }
func transfer(bal []int64, f, t int, amt, now int64, u *[]undo, e *[]ev) int {
	if !(amt > 0) { return Bad }
	if !(f != t) { return Same }
	if !(bal[f] >= amt) { return Insuf }
	nf := bal[f] - amt
	if amt > 0 && nf > bal[f] { return Ovf }
	*u = append(*u, undo{f, bal[f]}); bal[f] = nf
	if bal[t] > math.MaxInt64-amt { return Ovf }
	*u = append(*u, undo{t, bal[t]}); bal[t] += amt
	*e = append(*e, ev{1, uint32(f), uint32(t), amt, now})
	return Ok
}
var sink []ev
func run(bal []int64, calls, seed uint64) [5]uint64 {
	n := uint64(len(bal)); x := seed; now := int64(0); var st [5]uint64
	u := make([]undo, 0, 8); e := make([]ev, 0, 8)
	for i := uint64(0); i < calls; i++ {
		x ^= x << 13; x ^= x >> 7; x ^= x << 17
		f, t, amt := int(x%n), int((x>>20)%n), int64((x>>40)%1500)-10
		u = u[:0]; e = e[:0]
		o := transfer(bal, f, t, amt, now, &u, &e)
		if o != Ok { for j := len(u) - 1; j >= 0; j-- { bal[u[j].k] = u[j].v }; e = e[:0] }
		sink = e
		now++; st[o]++
	}
	return st
}
func rss() int64 { var r syscall.Rusage; syscall.Getrusage(0, &r); return r.Maxrss }
func main() {
	calls, _ := strconv.ParseUint(os.Args[1], 10, 64); threads := 0
	if len(os.Args) > 2 { threads, _ = strconv.Atoi(os.Args[2]) }
	r0 := rss(); bal := make([]int64, N); for i := range bal { bal[i] = 1000 }; r1 := rss()
	if threads == 0 {
		t0 := time.Now(); st := run(bal, calls, 88172645463325252); dt := time.Since(t0).Seconds()
		var cs int64; for i, b := range bal { cs += b * int64(i+1) }
		fmt.Printf("go ns/call=%.1f bytes/acct=%.1f ok=%d refused=%d checksum=%d\n", dt/float64(calls)*1e9, float64(r1-r0)/N, st[0], st[1]+st[2]+st[3], cs)
		return
	}
	per := calls / uint64(threads); chunk := N / threads; var wg sync.WaitGroup; t0 := time.Now()
	for i := 0; i < threads; i++ { wg.Add(1); go func(i int) { defer wg.Done(); run(bal[i*chunk:(i+1)*chunk], per, 88172645463325252+uint64(i)) }(i) }
	wg.Wait(); dt := time.Since(t0).Seconds()
	fmt.Printf("go threads=%d Mcalls/s=%.0f\n", threads, float64(per)*float64(threads)/dt/1e6)
}
