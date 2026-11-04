// What "Kern compiled straight to native" would emit for escrow_s2 `transfer`, plus a minimal runtime:
// interned accounts (dense u32 ids), i64 balances, checked arithmetic, undo log for atomic refusal,
// event buffer, command log with group commit (F_FULLFSYNC on macOS = real durability).
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/resource.h>
typedef struct { int64_t *bal; uint32_t *owner; int64_t now; } World;
typedef struct { uint32_t key; int64_t old; } Undo;
typedef struct { int ctor; int64_t a[4]; } Event;
enum { OK, BadAmount, SameAccount, Insufficient, Overflow };
static Undo undo[16]; static int nundo; static Event ev[16]; static int nev;
static inline void set(World *w, uint32_t k, int64_t v) { undo[nundo++] = (Undo){k, w->bal[k]}; w->bal[k] = v; }
static inline void rollback(World *w) { while (nundo) { nundo--; w->bal[undo[nundo].key] = undo[nundo].old; } nev = 0; }
// --- generated from the Kern definition ---
static int transfer(World *w, uint32_t from, uint32_t to, int64_t amount, int64_t *rarg) {
  if (!(amount > 0)) return BadAmount;
  if (!(from != to)) return SameAccount;
  if (!(w->bal[from] >= amount)) { *rarg = amount - w->bal[from]; return Insufficient; }
  int64_t nf, nt;
  if (__builtin_sub_overflow(w->bal[from], amount, &nf)) return Overflow;
  set(w, from, nf);
  if (__builtin_add_overflow(w->bal[to], amount, &nt)) return Overflow;
  set(w, to, nt);
  ev[nev++] = (Event){1, {from, to, amount, w->now}};
  return OK;
}
// --- runtime: atomic call wrapper ---
static int call(World *w, uint32_t f, uint32_t t, int64_t amt) {
  nundo = 0; nev = 0; int64_t r = 0;
  int out = transfer(w, f, t, amt, &r);
  if (out != OK) rollback(w);
  w->now++;
  return out;
}
static double secs(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec * 1e-9; }
int main(int argc, char **argv) {
  uint32_t N = 1000000; long CALLS = argc > 2 ? atol(argv[2]) : 20000000;
  struct rusage r0; getrusage(RUSAGE_SELF, &r0);
  World w = { malloc(N * sizeof(int64_t)), malloc(N * sizeof(uint32_t)), 0 };
  for (uint32_t i = 0; i < N; i++) { w.bal[i] = 1000; w.owner[i] = i; }
  struct rusage r1; getrusage(RUSAGE_SELF, &r1);
  uint64_t x = 88172645463325252ull; long st[5] = {0};
  int64_t total0 = 0; for (uint32_t i = 0; i < N; i++) total0 += w.bal[i];
  double t0 = secs();
  for (long i = 0; i < CALLS; i++) {
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    uint32_t f = x % N, t = (x >> 20) % N; int64_t amt = (int64_t)((x >> 40) % 1500) - 10;
    st[call(&w, f, t, amt)]++;
  }
  double dt = secs() - t0;
  int64_t total1 = 0; for (uint32_t i = 0; i < N; i++) total1 += w.bal[i];
  printf("in-memory: %.1f ns/call, %.1fM calls/s/core  ok=%ld refused=%ld  total kept=%d\n",
         dt / CALLS * 1e9, CALLS / dt / 1e6, st[OK], st[BadAmount] + st[SameAccount] + st[Insufficient], total0 == total1);
  { int64_t cs = 0; for (uint32_t i = 0; i < N; i++) cs += w.bal[i] * (int64_t)(i + 1); printf("c checksum=%lld ok=%ld\n", cs, st[OK]); }
  printf("memory: %.1f bytes/account (RSS delta for %u accounts)\n", (double)(r1.ru_maxrss - r0.ru_maxrss) / N, N);
  // durable: append each call (name id, args, now = 24 bytes) to a log, one F_FULLFSYNC per batch
  int fd = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0644);
  int batches[] = {1, 100, 1000, 10000};
  for (int b = 0; b < 4; b++) {
    long n = batches[b] == 1 ? 300 : 200000; int B = batches[b];
    char *buf = malloc((size_t)B * 24); double t1 = secs();
    for (long i = 0; i < n; i += B) {
      for (int j = 0; j < B; j++) {
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        uint32_t f = x % N, t = (x >> 20) % N; int64_t amt = (int64_t)((x >> 40) % 1500);
        call(&w, f, t, amt);
        memcpy(buf + j * 24, &f, 4); memcpy(buf + j * 24 + 4, &t, 4); memcpy(buf + j * 24 + 8, &amt, 8); memcpy(buf + j * 24 + 16, &w.now, 8);
      }
      write(fd, buf, (size_t)B * 24); fcntl(fd, F_FULLFSYNC);
    }
    double d = secs() - t1;
    printf("durable, %5d calls per sync: %8.0f calls/s, batch latency %.2f ms\n", B, n / d, d / (n / (double)B) * 1e3);
    free(buf);
  }
  close(fd);
}
