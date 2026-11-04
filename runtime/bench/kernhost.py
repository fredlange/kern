# Kern today: kern/host.py over in-memory SQLite (logs every call, outbox), running escrow_s2 `transfer`.
import sys, time, resource
sys.path.insert(0, "/Users/fredriklange/code/kern")
from kern.host import Host
from kern.syntax import parse_module
from kern.wf import check_module
N = 1_000_000; CALLS = int(sys.argv[1]); M = (1 << 64) - 1
defs = check_module(parse_module(open("/Users/fredriklange/code/kern/examples/escrow_s2.k").read()))
h = Host(defs, ":memory:", clock=iter(range(10**12)).__next__, sinks=[], gate=False)
r0 = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
h.db.execute("begin")
h.db.executemany("insert into account values(?,?,?)", ((f"A{i}", 1000, f"u{i}") for i in range(N)))
h.db.execute("commit")
r1 = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
run = h.interp.run; ti = [0.0]
def timed(*a):
    s = time.perf_counter(); r = run(*a); ti[0] += time.perf_counter() - s; return r
h.interp.run = timed
x = 88172645463325252; st = {}
t0 = time.perf_counter()
for _ in range(CALLS):
    x ^= (x << 13) & M; x ^= x >> 7; x ^= (x << 17) & M
    f = x % N; t = (x >> 20) % N; amt = (x >> 40) % 1500 - 10
    r = h.call("transfer", [f"A{f}", f"A{t}", amt]); st[r["status"]] = st.get(r["status"], 0) + 1
dt = time.perf_counter() - t0
cs = sum(b * (int(n[1:]) + 1) for n, b in h.db.execute("select name, balance from account"))
print(f"kern-host ns/call={dt/CALLS*1e9:.0f} interp-only ns/call={ti[0]/CALLS*1e9:.0f} bytes/acct(sqlite+py)={(r1-r0)/N:.1f} ok={st.get('ok',0)} refused={st.get('refused',0)} checksum={cs}")
