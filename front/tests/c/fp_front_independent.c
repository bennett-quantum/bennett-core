/* Native x86-64 is the oracle. Raw inputs cross volatile memory, not LLVM floating constants;
 * only the ten meaningful x87 bytes are returned. No runtime classifier is copied here. */
#include <math.h>
#include <stdint.h>
#include <string.h>
typedef unsigned __int128 U;

static const uint64_t fractions[] = {
  0, 1, 0x3fffffffffffffffULL, 0x4000000000000000ULL, 0x4000000000000001ULL,
  0x7fffffffffffffffULL, 0x8000000000000000ULL, 0x8000000000000001ULL,
  0xbfffffffffffffffULL, 0xc000000000000000ULL, 0xc000000000000001ULL, ~0ULL
};
static const unsigned exponents[] = {
  0, 1, 2, 0x3ffd, 0x3ffe, 0x3fff, 0x4000, 0x401d, 0x401e, 0x401f,
  0x403c, 0x403d, 0x403e, 0x403f, 0x407e, 0x7ffe, 0x7fff
};
static const int powers[] = {
  0, 1, -1, 2, -2, 3, -3, 17, -17, 64, -64, 65537, -65537,
  2147483647, -2147483647, (-2147483647 - 1)
};

unsigned inputs(unsigned width, unsigned boundary) {
  if (width != 2) return 36;
  return boundary ? 2 * (17 * 12 + 63 * 3) : 60;
}
unsigned power_count(void) { return sizeof(powers)/sizeof(*powers); }

static U encoding(unsigned width, unsigned i, unsigned boundary) {
  unsigned n = inputs(width, boundary) / 2, sign = i / n;
  i %= n;
  if (width != 2) {
    /* Every IEEE class, neighbors of one, rounding, under/overflow boundaries, NaN payloads. */
    static const uint64_t sf[] = {
      0, 1, 0x3fffff, 0x7fffff, 0x800000, 0x3effffff, 0x3f000000, 0x3f000001,
      0x3f7fffff, 0x3f800000, 0x3f800001, 0x3fc00000, 0x7f7fffff, 0x7f800000,
      0x7f800001, 0x7fbfffff, 0x7fc00000, 0x7fffffff
    };
    static const uint64_t df[] = {
      0, 1, 0x7ffffffffffffULL, 0xfffffffffffffULL, 0x10000000000000ULL,
      0x3fdfffffffffffffULL, 0x3fe0000000000000ULL, 0x3fe0000000000001ULL,
      0x3fefffffffffffffULL, 0x3ff0000000000000ULL, 0x3ff0000000000001ULL,
      0x3ff8000000000000ULL, 0x7fefffffffffffffULL, 0x7ff0000000000000ULL,
      0x7ff0000000000001ULL, 0x7ff7ffffffffffffULL, 0x7ff8000000000000ULL, 0x7fffffffffffffffULL
    };
    static const uint16_t hf[] = {
      0, 1, 0x1ff, 0x3ff, 0x400, 0x37ff, 0x3800, 0x3801, 0x3bff,
      0x3c00, 0x3c01, 0x3e00, 0x7bff, 0x7c00, 0x7c01, 0x7dff, 0x7e00, 0x7fff
    };
    static const uint16_t bf[] = {
      0, 1, 0x3f, 0x7f, 0x80, 0x3eff, 0x3f00, 0x3f01, 0x3f7f,
      0x3f80, 0x3f81, 0x3fc0, 0x7f7f, 0x7f80, 0x7f81, 0x7fbf, 0x7fc0, 0x7fff
    };
    if (width == 0) return sf[i] | (uint64_t)sign << 31;
    if (width == 1) return df[i] | (uint64_t)sign << 63;
    return (width == 3 ? hf[i] : bf[i]) | sign << 15;
  }
  unsigned e;
  uint64_t m;
  if (!boundary) {
    /* Canonical classes plus pseudo-denormals, unnormals, unnormal zeros and pseudo-NaNs/infinities. */
    static const unsigned es[] = {0, 1, 0x3fff, 0x7ffe, 0x7fff};
    static const uint64_t ms[] = {0, 1, 0x4000000000000000ULL,
      0x8000000000000000ULL, 0x8000000000000001ULL, 0xc000000000000001ULL};
    e = es[i / 6]; m = ms[i % 6];
  } else if (i < 17 * 12) {
    e = exponents[i / 12]; m = fractions[i % 12];
  } else {
    i -= 17 * 12;
    unsigned bit = i / 3;
    e = 0x3fff + bit;
    /* Below, at and above a half at every significand position, alternating the integral parity. */
    uint64_t half = 1ULL << (62 - bit);
    m = (0x8000000000000000ULL | half | (bit ? half << 1 : 0)) + i % 3 - 1;
  }
  return (U)(e | sign << 15) << 64 | m;
}

static U bits80(long double x) { U u = 0; memcpy(&u, &x, 10); return u; }

U power_result(unsigned width, unsigned i, unsigned p) {
  U u = encoding(width, i, 0);
  volatile int n = powers[p];
  if (width == 0) {
    float x; memcpy(&x, &u, 4); volatile float v = x;
    x = __builtin_powif(v, n); u = 0; memcpy(&u, &x, 4); return u;
  }
  if (width == 1) {
    double x; memcpy(&x, &u, 8); volatile double v = x;
    x = __builtin_powi(v, n); u = 0; memcpy(&u, &x, 8); return u;
  }
  long double x = 0; memcpy(&x, &u, 10); volatile long double v = x;
  return bits80(__builtin_powil(v, n));
}

U rounding_result(unsigned i, unsigned op) {
  U u = encoding(2, i, 1);
  long double x = 0; memcpy(&x, &u, 10); volatile long double v = x;
  switch (op) {
  case 0: return bits80(roundl(v));
  case 1: return bits80(__builtin_roundevenl(v));
  case 2: return bits80(rintl(v));
  case 3: return bits80(nearbyintl(v));
  case 4: return bits80(truncl(v));
  case 5: return bits80(floorl(v));
  case 6: return bits80(ceill(v));
  case 7: return (uint64_t)lrintl(v);
  case 8: return (uint64_t)llrintl(v);
  case 9: return (uint64_t)lroundl(v);
  case 10: return (uint64_t)llroundl(v);
  }
  return 0;
}

/* Different side-effecting noinline callees keep actual branches alive; the second field
 * uses a select with unrelated values, and the third returns the predicate itself. */
static volatile unsigned branch;
__attribute__((noinline)) static unsigned on_true(void) { branch = 0x93; return branch; }
__attribute__((noinline)) static unsigned on_false(void) { branch = 0x26; return branch; }
__attribute__((noinline)) static unsigned choose(unsigned t) { return t ? 0x57 : 0xa8; }
#define CLASSIFY(T) do { \
  T x; memcpy(&x, &u, sizeof(T)); volatile T v = x; \
  unsigned c; \
  switch (op) { \
  case 0: c = __builtin_isfinite(v); break; \
  case 1: c = __builtin_isnan(v); break; \
  case 2: c = __builtin_isinf(v) != 0; break; \
  case 3: c = __builtin_isnormal(v); break; \
  case 4: c = __builtin_fpclassify(13, 17, 23, 29, 31, v); break; \
  default: c = __builtin_signbit(v) != 0; break; \
  } \
  unsigned t = op == 4 ? c == 13 : c; \
  unsigned s = choose(t); \
  unsigned b; if (t) b = on_true(); else b = on_false(); \
  return ((U)c << 32) | ((U)s << 16) | b; \
} while (0)

U class_result(unsigned width, unsigned i, unsigned op) {
  U u = encoding(width, i, 0);
  switch (width) {
  case 0: CLASSIFY(float);
  case 1: CLASSIFY(double);
  case 2: CLASSIFY(long double);
  case 3: CLASSIFY(_Float16);
  default: CLASSIFY(__bf16);
  }
}

#ifdef NATIVE
#include <stdio.h>
static void print(U u) { printf("%016llx%016llx\n", (unsigned long long)(u >> 64), (unsigned long long)u); }
int main(int argc, char **argv) {
  if (argc != 2) return 2;
  if (argv[1][0] == 'p') {
    for (unsigned w = 0; w < 3; w++) for (unsigned i = 0; i < inputs(w, 0); i++)
      for (unsigned p = 0; p < power_count(); p++) print(power_result(w, i, p));
  } else if (argv[1][0] == 'r') {
    for (unsigned i = 0; i < inputs(2, 1); i++) for (unsigned op = 0; op < 11; op++) print(rounding_result(i, op));
  } else {
    for (unsigned w = 0; w < 5; w++) for (unsigned i = 0; i < inputs(w, 0); i++)
      for (unsigned op = 0; op < 6; op++) print(class_result(w, i, op));
  }
  return 0;
}
#endif
