# Hand-written Python in the style a Kern->Python compiler would emit (no interpreter).
import sys, time, resource
N = 1_000_000; CALLS = int(sys.argv[1]); M = (1 << 64) - 1
r0 = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
bal = [1000] * N
for i in range(N): bal[i] = 1000 + 0 * i      # distinct int objects, as after real updates
r1 = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
OK, BAD, SAME, INSUF, OVF = range(5)
LIM = (1 << 63) - 1
def transfer(f, t, amt, now, undo, ev):
    if not amt > 0: return BAD
    if not f != t: return SAME
    if not bal[f] >= amt: return INSUF
    nf = bal[f] - amt
    undo.append((f, bal[f])); bal[f] = nf
    nt = bal[t] + amt
    if nt > LIM: return OVF
    undo.append((t, bal[t])); bal[t] = nt
    ev.append((1, f, t, amt, now))
    return OK
x = 88172645463325252; now = 0; st = [0] * 5
t0 = time.perf_counter()
for _ in range(CALLS):
    x ^= (x << 13) & M; x ^= x >> 7; x ^= (x << 17) & M
    f = x % N; t = (x >> 20) % N; amt = (x >> 40) % 1500 - 10
    undo = []; ev = []
    o = transfer(f, t, amt, now, undo, ev)
    if o != OK:
        for k, v in reversed(undo): bal[k] = v
    now += 1; st[o] += 1
dt = time.perf_counter() - t0
print(f"plain-python ns/call={dt/CALLS*1e9:.0f} bytes/acct={(r1-r0)/N:.1f} ok={st[0]} refused={sum(st[1:4])} checksum={sum(b*(i+1) for i,b in enumerate(bal))}")
