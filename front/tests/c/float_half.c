/* float_half.c: _Float16 and __bf16 on directed values: arithmetic, square roots, fused multiply-adds,
 * every comparison, the classes, the rounding functions, fmin and fmax, signs, conversions to and from
 * float, double, long double, each other and every integer width up to 128 bits (out of range included:
 * undefined in C, and compared with what the native program gives). result(i) is the bit pattern of the
 * i-th result, results() how many there are; a 128-bit result is two, its high 64 bits and then its low 64.
 * Built with -DNATIVE, main prints them all. A result of two NaN operands is compared as "a NaN" only, as in
 * float_values.c.
 *
 * clang 18 cannot compile long double to __bf16 for x86-64 (its back end stops): natively that is the libgcc
 * routine a GCC build calls, __truncxfbf2. Between _Float16 and __bf16 clang 18 writes IR that LLVM rejects
 * (neither type is the wider), so they convert through float here. */

typedef unsigned long long u64;
typedef unsigned __int128 U;
typedef __int128 S;

#ifdef NATIVE
__bf16 __truncxfbf2(long double);
#define BF16_OF_X80(x) __truncxfbf2(x)
#else
#define BF16_OF_X80(x) ((__bf16)(x))
#endif

static _Float16 half(unsigned short b) {
  _Float16 r;
  __builtin_memcpy(&r, &b, 2);
  return r;
}
static __bf16 brain(unsigned short b) {
  __bf16 r;
  __builtin_memcpy(&r, &b, 2);
  return r;
}
static U hbits(_Float16 x) {
  unsigned short r;
  __builtin_memcpy(&r, &x, 2);
  return r;
}
static U bbits(__bf16 x) {
  unsigned short r;
  __builtin_memcpy(&r, &x, 2);
  return r;
}
static U fbits(float x) {
  unsigned r;
  __builtin_memcpy(&r, &x, 4);
  return r;
}
static U dbits(double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}
static U lbits(long double x) {
  u64 m = 0;
  unsigned short e = 0;
  __builtin_memcpy(&m, &x, 8);
  __builtin_memcpy(&e, (const char *)&x + 8, 2);
  return (U)e << 64 | m;
}
static int hnan(U b) { return (b & 0x7FFF) > 0x7C00; }
static int bnan(U b) { return (b & 0x7FFF) > 0x7F80; }

#define N 20
static volatile unsigned short HB[N] = {
    0x0000, 0x8000, 0x3C00, 0xBC00, 0x3E00, 0x2E66, 0x7BFF, 0xFBFF, 0x0400, 0x0001,
    0x03FF, 0x7C00, 0xFC00, 0x7E00, 0x7E55, 0x7C01, 0xFD23, 0x6800, 0x3C01, 0x3BFF,
};
static volatile unsigned short BB[N] = {
    0x0000, 0x8000, 0x3F80, 0xBF80, 0x3FC0, 0x3DCD, 0x7F7F, 0xFF7F, 0x0080, 0x0001,
    0x007F, 0x7F80, 0xFF80, 0x7FC0, 0x7FC5, 0x7F81, 0xFFA3, 0x4B80, 0x3F81, 0x3F7F,
};
static _Float16 H(unsigned i) { return half(HB[i]); }
static __bf16 B(unsigned i) { return brain(BB[i]); }

/* One operation per statement: C computes _Float16 and __bf16 expressions in float and rounds at the end. */
static U harith(unsigned op, unsigned i, unsigned j) {
  _Float16 a = H(i), b = H(j), r;
  switch (op) {
  case 0: r = a + b; break;
  case 1: r = a - b; break;
  case 2: r = a * b; break;
  default: r = a / b; break;
  }
  U bits = hbits(r);
  return hnan(hbits(a)) && hnan(hbits(b)) && hnan(bits) ? 1 : bits;
}
static U barith(unsigned op, unsigned i, unsigned j) {
  __bf16 a = B(i), b = B(j), r;
  switch (op) {
  case 0: r = a + b; break;
  case 1: r = a - b; break;
  case 2: r = a * b; break;
  default: r = a / b; break;
  }
  U bits = bbits(r);
  return bnan(bbits(a)) && bnan(bbits(b)) && bnan(bits) ? 1 : bits;
}

#define COMPARISONS(a, b)                                                                                  \
  ((U)((a) == (b)) | (U)((a) != (b)) << 1 | (U)((a) < (b)) << 2 | (U)((a) <= (b)) << 3 |                    \
   (U)((a) > (b)) << 4 | (U)((a) >= (b)) << 5 | (U)__builtin_isunordered(a, b) << 6 |                       \
   (U)__builtin_islessgreater(a, b) << 7)

#define CLASSES(x)                                                                                         \
  ((U)__builtin_isnan(x) | (U)__builtin_isinf(x) << 1 | (U)__builtin_isfinite(x) << 2 |                    \
   (U)__builtin_isnormal(x) << 3 | (U)(__builtin_signbit(x) != 0) << 4 |                                     \
   (U)__builtin_fpclassify(0, 1, 2, 3, 4, x) << 5 | (U)__builtin_isfpclass(x, 0x003) << 8 |                  \
   (U)__builtin_isfpclass(x, 0x090) << 9 | (U)__builtin_issignaling(x) << 10 |                              \
   (U)__builtin_isfpclass(x, 0x108) << 11)

/* The rounding functions, signs, fmin, fmax, sqrt and fma on one value (and the next ones). */
#define UNARY 15
static U hunary(unsigned k, unsigned i) {
  _Float16 x = H(i), y = H((i + 7) % N), z = H((i + 3) % N), r;
  switch (k) {
  case 0: r = __builtin_floorf16(x); break;
  case 1: r = __builtin_ceilf16(x); break;
  case 2: r = __builtin_truncf16(x); break;
  case 3: r = __builtin_roundf16(x); break;
  case 4: r = __builtin_rintf16(x); break;
  case 5: r = (_Float16)__builtin_nearbyintf(x); break;
  case 6: r = __builtin_roundevenf16(x); break;
  case 7: r = __builtin_fabsf16(x); break;
  case 8: r = -x; break;
  case 9: r = __builtin_copysignf16(x, y); break;
  case 10: r = __builtin_sqrtf16(x); break;
  case 11: r = __builtin_fminf16(x, y); break;
  case 12: r = __builtin_fmaxf16(x, y); break;
  case 13: r = __builtin_fmaf16(x, y, z); break;
  default: return __builtin_fabsf16(x) < 1e4f16 ? (u64)__builtin_lrintf((float)x) : 0;
  }
  U bits = hbits(r);
  int nans = hnan(hbits(x)) + (k >= 9 && hnan(hbits(y))) + (k == 13 && hnan(hbits(z)));
  return k >= 11 && nans >= 2 && hnan(bits) ? 1 : bits;
}
static U bunary(unsigned k, unsigned i) {
  __bf16 x = B(i), y = B((i + 7) % N), z = B((i + 3) % N), r;
  switch (k) {
  case 0: r = (__bf16)__builtin_floorf(x); break;
  case 1: r = (__bf16)__builtin_ceilf(x); break;
  case 2: r = (__bf16)__builtin_truncf(x); break;
  case 3: r = (__bf16)__builtin_roundf(x); break;
  case 4: r = (__bf16)__builtin_rintf(x); break;
  case 5: r = (__bf16)__builtin_nearbyintf(x); break;
  case 6: r = (__bf16)__builtin_roundevenf(x); break;
  case 7: r = (__bf16)__builtin_fabsf(x); break;
  case 8: r = -x; break;
  case 9: r = (__bf16)__builtin_copysignf(x, y); break;
  case 10: r = (__bf16)__builtin_sqrtf(x); break;
  case 11: r = (__bf16)__builtin_fminf(x, y); break;
  case 12: r = (__bf16)__builtin_fmaxf(x, y); break;
  case 13: r = (__bf16)__builtin_fmaf(x, y, z); break;
  default: return (float)x < 1e4f && (float)x > -1e4f ? (u64)__builtin_lrintf(x) : 0;
  }
  U bits = bbits(r);
  int nans = bnan(bbits(x)) + (k >= 9 && bnan(bbits(y))) + (k == 13 && bnan(bbits(z)));
  return k >= 11 && nans >= 2 && bnan(bits) ? 1 : bits;
}

/* To _Float16 and __bf16 from wider formats: ties, sticky bits, subnormal results, overflow, NaNs. */
#define NW 18
static volatile long double W[NW] = {
    1.0L + 0x1p-11L,              /* half's tie, to even (1) */
    1.0L + 0x1p-11L + 0x1p-40L,   /* just above: up */
    1.0L + 0x3p-11L,              /* half's tie, to even (up) */
    1.0L + 0x1p-8L,               /* __bf16's tie */
    1.0L + 0x1p-8L + 0x1p-30L,    /* just above, within float: double rounding would lose it */
    1.0L + 0x1p-8L + 0x1p-60L,    /* just above, below float's precision */
    1.0L + 0x1p-8L - 0x1p-60L,    /* just below */
    65504.0L,                     /* half's greatest */
    65520.0L,                     /* half's overflow tie: to infinity */
    65519.99L,                    /* just below: to the greatest */
    0x1p-24L,                     /* half's least subnormal */
    0x1p-25L,                     /* half of it: a tie, to 0 */
    0x1.8p-25L,                   /* up */
    0x1.fep127L,                  /* __bf16's greatest */
    0x1.ffp127L,                  /* __bf16's overflow tie: to infinity */
    0x1p-133L + 0x1p-134L,        /* a __bf16 subnormal tie */
    0x1p-134L + 0x1p-200L,        /* just above half of __bf16's least: up */
    0.0L / 0.0L,
};

static U narrow(unsigned kind, unsigned i) {
  long double x = W[i];
  double d = (double)x;
  float f = (float)x;
  switch (kind) {
  case 0: return hbits((_Float16)f);
  case 1: return hbits((_Float16)d);
  case 2: return hbits((_Float16)x);
  case 3: return bbits((__bf16)f);
  case 4: return bbits((__bf16)d);
  case 5: return bbits(BF16_OF_X80(x));
  case 6: return hbits((_Float16)-d);
  default: return bbits((__bf16)-d);
  }
}

/* From _Float16 and __bf16: to every wider format and to each other. */
static U widen(unsigned kind, unsigned i) {
  _Float16 h = H(i);
  __bf16 b = B(i);
  switch (kind) {
  case 0: return fbits((float)h);
  case 1: return dbits((double)h);
  case 2: return lbits((long double)h);
  case 3: return bbits((__bf16)(float)h);
  case 4: return fbits((float)b);
  case 5: return dbits((double)b);
  case 6: return lbits((long double)b);
  default: return hbits((_Float16)(float)b);
  }
}

/* To integers: values in and out of range of each width. */
#define NI 12
static volatile float TOI[NI] = {0.9f, -0.9f, 127.5f, -129.0f, 255.5f, 300.0f, -40000.0f, 60000.0f,
                                 65504.0f, -1.0f, 3e9f, 1e30f};
static U to_integer(unsigned kind, unsigned i) {
  _Float16 h = (_Float16)TOI[i];
  __bf16 b = (__bf16)TOI[i];
  switch (kind) {
  case 0: return (U)(S)(signed char)h;
  case 1: return (unsigned char)h;
  case 2: return (U)(S)(short)h;
  case 3: return (unsigned short)h;
  case 4: return (U)(S)(int)h;
  case 5: return (unsigned)h;
  case 6: return (U)(S)(long long)h;
  case 7: return (unsigned long long)h;
  case 8: return (U)(S)h;
  case 9: return (U)h;
  case 10: return (U)(S)(signed char)b;
  case 11: return (unsigned char)b;
  case 12: return (U)(S)(int)b;
  case 13: return (unsigned)b;
  case 14: return (U)(S)(long long)b;
  case 15: return (unsigned long long)b;
  case 16: return (U)(S)b;
  default: return (U)b;
  }
}
/* And of the infinities and NaNs of both. */
static U special_to_integer(unsigned kind, unsigned i) {
  static const unsigned short hs[4] = {0x7C00, 0xFC00, 0x7E00, 0xFE00}, bs[4] = {0x7F80, 0xFF80, 0x7FC0, 0xFFC0};
  _Float16 h = half(hs[i]);
  __bf16 b = brain(bs[i]);
  switch (kind) {
  case 0: return (U)(S)h;
  case 1: return (U)h;
  case 2: return (U)(S)b;
  default: return (U)b;
  }
}

/* From integers: ties for _Float16 and for __bf16 (whose conversion from an integer rounds twice, through
 * float, as the native program does), overflow, the 128-bit edges. */
#define NJ 12
static volatile u64 JH[NJ] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFFFFFFFFFFFFFFFF, 0x8000000000000000, 1};
static volatile u64 JL[NJ] = {
    2049,                 /* half's tie at 2^11: to even */
    2051,                 /* to even, up */
    65519,                /* below half's overflow tie */
    65520,                /* half's overflow tie */
    0x1010001,            /* 2^24 + 2^16 + 1: through float a tie, so __bf16 rounds down; exactly, up */
    0x80000080,           /* __bf16's tie at 2^31 */
    0xFFFFFFFFFFFFFFFF,
    0x8000008000000001,
    0x7FFFFFFF,
    0xFFFFFFFFFFFFFFFF,   /* -1 */
    0,                    /* the least 128-bit integer */
    0x0000800000000000,
};
static U from_integer(unsigned kind, unsigned i) {
  U v = (U)JH[i] << 64 | JL[i];
  switch (kind) {
  case 0: return hbits((_Float16)(int)v);
  case 1: return hbits((_Float16)(unsigned)v);
  case 2: return hbits((_Float16)(long long)v);
  case 3: return hbits((_Float16)(unsigned long long)v);
  case 4: return hbits((_Float16)(S)v);
  case 5: return hbits((_Float16)v);
  case 6: return bbits((__bf16)(int)v);
  case 7: return bbits((__bf16)(unsigned)v);
  case 8: return bbits((__bf16)(long long)v);
  case 9: return bbits((__bf16)(unsigned long long)v);
  case 10: return bbits((__bf16)(S)v);
  default: return bbits((__bf16)v);
  }
}

#define ARITH (4 * N * N)
#define COMPARE (N * N)
#define CLASS N
#define UNARIES (UNARY * N)
#define NARROW (8 * NW)
#define WIDEN (8 * N)
#define TO_INT (18 * NI)
#define SPECIAL (4 * 4)
#define FROM_INT (12 * NJ)
#define FMT_COUNT (2 * ARITH + 2 * COMPARE + 2 * CLASS + 2 * UNARIES)

static U value(unsigned i) {
  if (i < ARITH) return harith(i / (N * N), i / N % N, i % N);
  i -= ARITH;
  if (i < ARITH) return barith(i / (N * N), i / N % N, i % N);
  i -= ARITH;
  if (i < COMPARE) {
    _Float16 a = H(i / N), b = H(i % N);
    return COMPARISONS(a, b);
  }
  i -= COMPARE;
  if (i < COMPARE) {
    __bf16 a = B(i / N), b = B(i % N);
    return COMPARISONS(a, b);
  }
  i -= COMPARE;
  if (i < CLASS) {
    _Float16 x = H(i);
    return CLASSES(x);
  }
  i -= CLASS;
  if (i < CLASS) {
    __bf16 x = B(i);
    return CLASSES(x);
  }
  i -= CLASS;
  if (i < UNARIES) return hunary(i / N, i % N);
  i -= UNARIES;
  if (i < UNARIES) return bunary(i / N, i % N);
  i -= UNARIES;
  if (i < NARROW) return narrow(i / NW, i % NW);
  i -= NARROW;
  if (i < WIDEN) return widen(i / N, i % N);
  i -= WIDEN;
  if (i < TO_INT) return to_integer(i / NI, i % NI);
  i -= TO_INT;
  if (i < SPECIAL) return special_to_integer(i / 4, i % 4);
  i -= SPECIAL;
  return from_integer(i / NJ, i % NJ);
}

unsigned results(void) { return 2 * (FMT_COUNT + NARROW + WIDEN + TO_INT + SPECIAL + FROM_INT); }
u64 result(unsigned i) {
  U r = value(i / 2);
  return i % 2 ? (u64)r : (u64)(r >> 64);
}

#ifdef NATIVE
int printf(const char *, ...);
int main(void) {
  for (unsigned i = 0; i < results(); i++) {
    printf("%llu\n", result(i));
  }
  return 0;
}
#endif
