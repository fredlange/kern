// Rust in the style a Kern->Rust compiler would emit: checked i64, undo log, event buffer.
use std::time::Instant;
const N: usize = 1_000_000;
#[derive(Clone, Copy, PartialEq)] enum Out { Ok, Bad, Same, Insuf, Ovf }
struct Ev { _c: u8, _f: u32, _t: u32, _a: i64, _now: i64 }
#[repr(C)] struct RUsage { t: [i64; 4], maxrss: i64, rest: [i64; 13] }
extern "C" { fn getrusage(who: i32, r: *mut RUsage) -> i32; }
fn maxrss() -> i64 { unsafe { let mut r: RUsage = std::mem::zeroed(); getrusage(0, &mut r); r.maxrss } }
#[inline(always)]
fn transfer(bal: &mut [i64], f: usize, t: usize, amt: i64, now: i64, undo: &mut Vec<(usize, i64)>, ev: &mut Vec<Ev>) -> Out {
    if !(amt > 0) { return Out::Bad; }
    if !(f != t) { return Out::Same; }
    if !(bal[f] >= amt) { return Out::Insuf; }
    let Some(nf) = bal[f].checked_sub(amt) else { return Out::Ovf };
    undo.push((f, bal[f])); bal[f] = nf;
    let Some(nt) = bal[t].checked_add(amt) else { return Out::Ovf };
    undo.push((t, bal[t])); bal[t] = nt;
    ev.push(Ev { _c: 1, _f: f as u32, _t: t as u32, _a: amt, _now: now });
    Out::Ok
}
#[inline(always)] fn xs(x: &mut u64) -> u64 { *x ^= *x << 13; *x ^= *x >> 7; *x ^= *x << 17; *x }
// one shard: its own accounts, its own call stream (zero cross-shard calls = scheduler's best case)
fn run(bal: &mut [i64], calls: u64, seed: u64) -> [u64; 5] {
    let n = bal.len() as u64; let mut x = seed; let mut now = 0i64; let mut st = [0u64; 5];
    let mut undo = Vec::with_capacity(8); let mut ev = Vec::with_capacity(8);
    for _ in 0..calls {
        let r = xs(&mut x);
        let (f, t, amt) = ((r % n) as usize, ((r >> 20) % n) as usize, ((r >> 40) % 1500) as i64 - 10);
        undo.clear(); ev.clear();
        let o = transfer(bal, f, t, amt, now, &mut undo, &mut ev);
        if o != Out::Ok { while let Some((k, v)) = undo.pop() { bal[k] = v; } ev.clear(); }
        std::hint::black_box(&ev);
        now += 1; st[o as usize] += 1;
    }
    st
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let calls: u64 = a[1].parse().unwrap(); let threads: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(0);
    let r0 = maxrss(); let mut bal = vec![1000i64; N]; for b in bal.iter_mut() { *b = std::hint::black_box(1000); } let r1 = maxrss();
    if threads == 0 {
        let t0 = Instant::now(); let st = run(&mut bal, calls, 88172645463325252); let dt = t0.elapsed().as_secs_f64();
        let cs: i64 = bal.iter().enumerate().map(|(i, b)| b * (i as i64 + 1)).sum();
        println!("rust ns/call={:.1} bytes/acct={:.1} ok={} refused={} checksum={}", dt / calls as f64 * 1e9, (r1 - r0) as f64 / N as f64, st[0], st[1] + st[2] + st[3], cs);
    } else {
        let per = calls / threads as u64; let chunk = N / threads; let t0 = Instant::now();
        std::thread::scope(|s| { for (i, c) in bal.chunks_mut(chunk).enumerate() { s.spawn(move || run(c, per, 88172645463325252 + i as u64)); } });
        let dt = t0.elapsed().as_secs_f64();
        println!("rust threads={} Mcalls/s={:.0}", threads, per as f64 * threads as f64 / dt / 1e6);
    }
}
