/* Tracing runtime for the rewritten SQLite IR (see rewrite_ir.py).
 * Compiled WITHOUT instrumentation. Only active between __tr_begin and __tr_end,
 * which the driver calls in a forked child around the region.
 *
 * Per branch r it writes into DIR:
 *   acc.r.bin  one 24-byte record per memory access, in program order
 *   blk.r.bin  one u32 block id per executed basic block, in order
 *   wr.r.bin   per 64-byte line the region wrote: line address, byte mask,
 *              initial 64 bytes, final 64 bytes (144 bytes per line)
 *   sum.r.txt  counters
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

enum { F_ST = 1u << 28, F_MEM = 2u << 28, SITE_MASK = (1u << 28) - 1 };

typedef struct { uint32_t site_fl, size; uint64_t addr, keyb; } Rec;
typedef struct { uint64_t line, mask; uint8_t init[64]; } WLine;
typedef struct { uint64_t frame, x; uint32_t cnt, used; } OrdSlot;
typedef struct { uint64_t path, id; } Frame;

/* Capacities are space limits, not time; the run aborts loudly if one is hit. */
#define ACC_CAP (1ull << 28)
#define BLK_CAP (1ull << 29)
#define WL_BITS 20
#define ORD_BITS 25
#define MAX_DEPTH 4096

int __tr_on;
static Rec *acc; static uint64_t nacc;
static uint32_t *blk; static uint64_t nblk;
static WLine *wl; static uint64_t nwl;
static OrdSlot *ord; static uint64_t nord;
static Frame stk[MAX_DEPTH]; static int sp, maxsp;
static uint64_t frame_ctr, ninstr, nld, nst, nmem, bld, bst;

static void die(const char *m) { fprintf(stderr, "tracer: %s\n", m); _exit(3); }

static void *bigmap(uint64_t bytes) {
  void *p = mmap(0, bytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  if (p == MAP_FAILED) die("mmap");
  return p;
}

static inline uint64_t mix(uint64_t a, uint64_t b) {
  uint64_t z = a * 0x9E3779B97F4A7C15ull ^ (b + 0x632BE59BD9B4E019ull + (a << 6) + (a >> 2));
  z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
  z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
  return z ^ (z >> 31);
}

static uint32_t bump(uint64_t frame, uint64_t x) {
  uint64_t m = (1ull << ORD_BITS) - 1, h = mix(frame, x) & m;
  for (;;) {
    OrdSlot *s = &ord[h];
    if (!s->used) {
      if (++nord > (m * 7) / 10) die("ordinal table full");
      s->used = 1; s->frame = frame; s->x = x; s->cnt = 1; return 0;
    }
    if (s->frame == frame && s->x == x) return s->cnt++;
    h = (h + 1) & m;
  }
}

static WLine *wline(uint64_t line) {
  uint64_t m = (1ull << WL_BITS) - 1, h = mix(line, 0) & m;
  for (;;) {
    WLine *w = &wl[h];
    if (!w->line) {
      if (++nwl > (m * 7) / 10) die("write table full");
      w->line = line; w->mask = 0; return w;
    }
    if (w->line == line) return w;
    h = (h + 1) & m;
  }
}

static void note_write(uint64_t a, uint64_t n) {
  for (uint64_t p = a; p < a + n; p++) {
    WLine *w = wline(p & ~63ull);
    unsigned o = p & 63;
    if (!(w->mask >> o & 1)) { w->init[o] = *(volatile uint8_t *)p; w->mask |= 1ull << o; }
  }
}

static inline void rec(uint32_t site_fl, uint64_t size, const void *p) {
  if (nacc >= ACC_CAP) die("access buffer full");
  uint32_t key = site_fl;
  uint64_t o = bump(stk[sp].id, key);
  acc[nacc++] = (Rec){site_fl, (uint32_t)size, (uint64_t)p, mix(mix(stk[sp].path, key), o)};
}

void __tr_ld(void *p, uint32_t site, uint32_t size) {
  if (!__tr_on) return;
  nld++; bld += size; rec(site, size, p);
}
void __tr_st(void *p, uint32_t site, uint32_t size) {
  if (!__tr_on) return;
  nst++; bst += size; note_write((uint64_t)p, size); rec(site | F_ST, size, p);
}
void __tr_bb(uint32_t id, uint32_t ni) {
  if (!__tr_on) return;
  if (nblk >= BLK_CAP) die("block buffer full");
  blk[nblk++] = id; ninstr += ni;
}

static void mload(uint32_t site, const void *p, uint64_t n) { nld++; bld += n; rec(site | F_MEM, n, p); }
static void mstore(uint32_t site, void *p, uint64_t n) {
  nst++; bst += n; note_write((uint64_t)p, n); rec(site | F_MEM | F_ST, n, p);
}
static uint64_t cmp_len(const unsigned char *a, const unsigned char *b, uint64_t n, int stop_nul) {
  uint64_t i = 0;
  for (; i < n; i++) {
    if (a[i] != b[i]) return i + 1;
    if (stop_nul && !a[i]) return i + 1;
  }
  return n;
}

void __tr_mem(void *a, void *b, uint64_t n, uint32_t site, uint32_t kind) {
  if (!__tr_on) return;
  nmem++;
  const unsigned char *ua = a, *ub = b;
  uint64_t m;
  switch (kind) {
  case 0: mload(site, b, n); mstore(site, a, n); break;               /* memcpy/memmove */
  case 1: mstore(site, a, n); break;                                   /* memset */
  case 2: m = cmp_len(ua, ub, n, 0); mload(site, a, m); mload(site, b, m); break;
  case 3: mload(site, a, strlen(a) + 1); break;                       /* strlen */
  case 4: m = cmp_len(ua, ub, UINT64_MAX, 1); mload(site, a, m); mload(site, b, m); break;
  case 5: m = cmp_len(ua, ub, n, 1); mload(site, a, m); mload(site, b, m); break;
  case 6: { const unsigned char *f = memchr(a, (int)(uintptr_t)b, n);  /* memchr */
            mload(site, a, f ? (uint64_t)(f - ua) + 1 : n); break; }
  case 7: { const char *f = strchr(a, (int)(uintptr_t)b);             /* strchr */
            mload(site, a, f ? (uint64_t)(f - (const char *)a) + 1 : strlen(a) + 1); break; }
  case 8: mload(site, a, strlen(a) + 1); break;                       /* strrchr */
  case 9: mload(site, a, strcspn(a, b) + 1); mload(site, b, strlen(b) + 1); break;
  case 10: mload(site, a, strspn(a, b) + 1); mload(site, b, strlen(b) + 1); break;
  default: die("unknown mem kind");
  }
}

void __cyg_profile_func_enter(void *fn, void *cs) {
  (void)fn;
  if (!__tr_on) return;
  if (sp + 1 >= MAX_DEPTH) die("call depth");
  uint64_t o = bump(stk[sp].id, (uint64_t)cs | (1ull << 63));
  Frame f = {mix(mix(stk[sp].path, (uint64_t)cs), o), ++frame_ctr};
  stk[++sp] = f;
  if (sp > maxsp) maxsp = sp;
}
void __cyg_profile_func_exit(void *fn, void *cs) {
  (void)fn; (void)cs;
  if (!__tr_on) return;
  if (sp == 0) die("unbalanced exit");
  sp--;
}

/* trace-pc-guard was only enabled to switch the coverage pass on; its calls are removed. */
void __sanitizer_cov_trace_pc_guard_init(uint32_t *a, uint32_t *b) { (void)a; (void)b; }

void __tr_begin(void) {
  acc = bigmap(ACC_CAP * sizeof(Rec));
  blk = bigmap(BLK_CAP * sizeof(uint32_t));
  wl = bigmap((1ull << WL_BITS) * sizeof(WLine));
  ord = bigmap((1ull << ORD_BITS) * sizeof(OrdSlot));
  sp = 0; stk[0] = (Frame){0, ++frame_ctr};
  __tr_on = 1;
}

static void wfile(const char *dir, const char *kind, int r, const void *p, uint64_t bytes) {
  char path[4096];
  snprintf(path, sizeof path, "%s/%s.%d.bin", dir, kind, r);
  int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (fd < 0) die("open output");
  const char *c = p;
  while (bytes) {
    ssize_t w = write(fd, c, bytes);
    if (w <= 0) die("write");
    c += w; bytes -= (uint64_t)w;
  }
  close(fd);
}

void __tr_end(const char *dir, int r, long long result, int rc) {
  __tr_on = 0;
  if (sp != 0) die("region ended with open frames");
  wfile(dir, "acc", r, acc, nacc * sizeof(Rec));
  wfile(dir, "blk", r, blk, nblk * sizeof(uint32_t));
  /* written lines: line, mask, initial[64], final[64] */
  uint64_t cap = 1ull << WL_BITS, k = 0;
  uint8_t *out = malloc(nwl * 144 + 1);
  for (uint64_t i = 0; i < cap; i++) {
    WLine *w = &wl[i];
    if (!w->line) continue;
    uint8_t *o = out + k++ * 144;
    memcpy(o, &w->line, 8); memcpy(o + 8, &w->mask, 8); memcpy(o + 16, w->init, 64);
    for (int b = 0; b < 64; b++) o[80 + b] = (w->mask >> b & 1) ? *(volatile uint8_t *)(w->line + b) : 0;
  }
  wfile(dir, "wr", r, out, k * 144);
  char path[4096];
  snprintf(path, sizeof path, "%s/sum.%d.txt", dir, r);
  FILE *f = fopen(path, "w");
  if (!f) die("open sum");
  fprintf(f, "r=%d rc=%d result=%lld blocks=%llu llvm_instr=%llu loads=%llu stores=%llu memops=%llu "
             "load_bytes=%llu store_bytes=%llu records=%llu written_lines=%llu max_depth=%d\n",
          r, rc, result, (unsigned long long)nblk, (unsigned long long)ninstr, (unsigned long long)nld,
          (unsigned long long)nst, (unsigned long long)nmem, (unsigned long long)bld,
          (unsigned long long)bst, (unsigned long long)nacc, (unsigned long long)k, maxsp);
  fclose(f);
}
