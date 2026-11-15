/* float_long_double.c: long double (the x87 80-bit format) on directed values: arithmetic, square roots,
 * unfused and fused multiply-adds, every comparison, the classes, conversions to and from float, double and every
 * integer width (out of range included: undefined in C, and compared with what the native program's x87
 * code gives), rounding, fmin and fmax, powers. Values are written as their sign-and-exponent and
 * significand (with the explicit integer bit), the order of x87's memory format. result(i) is the bit
 * pattern of the i-th result, results() how many there are; an 80-bit result is two, its high 64 bits
 * and then its low 64 bits (C returns a 128-bit integer in two registers, which the device program does
 * not). Built with -DNATIVE, main prints them all. See float_values.c for how the results are checked, and why a result
 * of two NaN operands is compared as "a NaN" only. */

typedef unsigned long long u64;
typedef unsigned __int128 U;

static long double ld(unsigned short e, u64 m) {
  long double r = 0;
  __builtin_memcpy(&r, &m, 8);
  __builtin_memcpy((char *)&r + 8, &e, 2);
  return r;
}
static U lbits(long double x) {
  u64 m = 0;
  unsigned short e = 0;
  __builtin_memcpy(&m, &x, 8);
  __builtin_memcpy(&e, (const char *)&x + 8, 2);
  return (U)e << 64 | m;
}
static u64 dbits(double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}
static u64 fbits(float x) {
  unsigned r;
  __builtin_memcpy(&r, &x, 4);
  return r;
}
static int lnan(U b) { return (b >> 64 & 0x7FFF) == 0x7FFF && (u64)b << 1 != 0; }

#define NX 22
static volatile unsigned short XE[NX] = {
    0x0000, 0x8000, 0x3FFF, 0xBFFF, 0x3FFB, 0x4000, 0x3FFF, 0x403F, 0xC03E, 0x0000, 0x0000,
    0x0001, 0x7FFE, 0x7FFF, 0xFFFF, 0x7FFF, 0x7FFF, 0xFFFF, 0x3FBF, 0x3FFE, 0x403E, 0x43FE,
};
static volatile u64 XM[NX] = {
    0x0000000000000000, /* 0 */
    0x0000000000000000, /* -0 */
    0x8000000000000000, /* 1 */
    0x8000000000000000, /* -1 */
    0xCCCCCCCCCCCCCCCD, /* 0.1 */
    0xC000000000000000, /* 3 */
    0x8000000000000001, /* 1 + 2^-63 */
    0x8000000000000000, /* 2^64 */
    0xFFFFFFFFFFFFFFFF, /* -(2^64 - 1) */
    0x0000000000000001, /* the least subnormal */
    0x7FFFFFFFFFFFFFFF, /* the greatest subnormal */
    0x8000000000000000, /* the least normal */
    0xFFFFFFFFFFFFFFFF, /* the greatest finite */
    0x8000000000000000, /* inf */
    0x8000000000000000, /* -inf */
    0xC000000000000123, /* a quiet NaN with a payload */
    0x8000000000000456, /* a signalling NaN */
    0xC000000000000000, /* x87's default NaN */
    0x8000000000000000, /* 2^-64: half an ulp of 1, a tie */
    0xC000000000000000, /* 0.75 */
    0x8000000000000000, /* 2^63 */
    0x8000000000000000, /* 2^1023, which double holds, and whose square it does not */
};
static long double X(unsigned i) { return ld(XE[i], XM[i]); }

static U arith(unsigned op, unsigned i, unsigned j) {
  long double a = X(i), b = X(j), r;
  switch (op) {
  case 0: r = a + b; break;
  case 1: r = a - b; break;
  case 2: r = a * b; break;
  default: r = a / b; break;
  }
  U bits = lbits(r);
  return lnan(lbits(a)) && lnan(lbits(b)) && lnan(bits) ? 1 : bits;
}

#define COMPARISONS(a, b)                                                                                  \
  ((U)((a) == (b)) | (U)((a) != (b)) << 1 | (U)((a) < (b)) << 2 | (U)((a) <= (b)) << 3 |                    \
   (U)((a) > (b)) << 4 | (U)((a) >= (b)) << 5 | (U)!((a) < (b)) << 6 | (U)!((a) <= (b)) << 7 |              \
   (U)!((a) > (b)) << 8 | (U)!((a) >= (b)) << 9 | (U)__builtin_isunordered(a, b) << 10 |                    \
   (U)!__builtin_isunordered(a, b) << 11 | (U)__builtin_islessgreater(a, b) << 12 |                         \
   (U)((a) == (b) || __builtin_isunordered(a, b)) << 13 | (U)__builtin_isless(a, b) << 14 |                 \
   (U)__builtin_isgreaterequal(a, b) << 15 | (U)((a) < (b) ? 1 : 0) << 16)

#define CLASSES(x)                                                                                         \
  ((U)__builtin_isnan(x) | (U)__builtin_isinf(x) << 1 | (U)__builtin_isfinite(x) << 2 |                    \
   (U)__builtin_isnormal(x) << 3 | (U)(__builtin_signbit(x) != 0) << 4 |                                     \
   (U)__builtin_fpclassify(0, 1, 2, 3, 4, x) << 5 | (U)__builtin_isfpclass(x, 0x003) << 8 |                  \
   (U)__builtin_isfpclass(x, 0x204) << 9 | (U)__builtin_isfpclass(x, 0x108) << 10 |                         \
   (U)__builtin_isfpclass(x, 0x090) << 11 | (U)__builtin_isfpclass(x, 0x060) << 12 |                        \
   (U)__builtin_isfpclass(x, 0x001) << 13 | (U)__builtin_isfpclass(x, 0x2AA) << 14 |                        \
   (U)__builtin_issignaling(x) << 15 | (U)__builtin_isfpclass(x, 0x002) << 16 |                             \
   (U)__builtin_isfpclass(x, 0x100) << 17 | (U)__builtin_isfpclass(x, 0x008) << 18)

/* Triples for a * b + c, unfused (llvm.fmuladd), as natively. */
#define NT 6
static const unsigned T[NT][3] = {{4, 7, 3}, {6, 6, 3}, {12, 5, 12}, {13, 0, 2}, {2, 2, 15}, {1, 0, 1}};

/* To double and float: values whose rounding is interesting (ties, double rounding, overflow,
 * subnormal results), then the directed ones. */
#define NN 12
static volatile unsigned short NE[NN] = {0x3FFF, 0x3FFF, 0x3FFF, 0x3FFF, 0x3C01, 0x3BCD,
                                          0x3BCC, 0x43FF, 0xC3FE, 0x407E, 0x3F69, 0x0001};
static volatile u64 NM[NN] = {
    0x8000000000000400, /* 1 + 2^-53: a tie, to even (1) */
    0x8000000000000401, /* just above the tie: up */
    0x8000000000000C00, /* 1 + 3 * 2^-53: a tie, to even (up) */
    0x80000080000000FF, /* 1 + 2^-24 + a little: float rounds up */
    0xC000000000000000, /* 1.5 * 2^-1022 */
    0xC000000000000000, /* 1.5 * 2^-1074: a subnormal tie, to even (2^-1073) */
    0x8000000000000000, /* 2^-1075: half the least subnormal, to 0 */
    0x8000000000000000, /* 2^1024: overflow to inf */
    0xFFFFFFFFFFFFFFFF, /* below -max double: overflows */
    0xFFFFFF7FFFFFFFFF, /* below max float + half an ulp: float rounds to max float */
    0xC000000000000000, /* 1.5 * 2^-150: a float subnormal tie */
    0x8000000000000000, /* the least normal: 0 in both */
};

/* From double and float: directed values. */
#define ND 10
static volatile u64 D[ND] = {
    0x0000000000000000, 0x8000000000000000, 0x3FB999999999999A, 0x0000000000000001, 0x000FFFFFFFFFFFFF,
    0x7FEFFFFFFFFFFFFF, 0xFFF0000000000000, 0x7FF8000000000123, 0x7FF0000000000456, 0xFFF8000000000000,
};
static volatile unsigned F[ND] = {0x00000000, 0x80000000, 0x3DCCCCCD, 0x00000001, 0x007FFFFF,
                                  0x7F7FFFFF, 0xFF800000, 0x7FC00123, 0x7F800456, 0xFFC00000};

/* To integers, in range and out of it. */
#define NC 26
static volatile long double TO[NC] = {
    0.0L,          -0.0L,         0.9L,       -0.9L,        1.5L,        -1.5L,      127.99L,
    -128.99L,      255.99L,       300.0L,     -200.0L,      32767.9L,    -32768.9L,  65535.9L,
    65536.0L,      -40000.0L,     1e10L,      -4294967296.0L, 4294967295.5L, 9223372036854775807.0L,
    9223372036854775808.0L, 18446744073709551615.0L, 18446744073709551616.0L, -9223372036854775809.0L,
    1e30L,         -1e30L,
};

/* From long double, then from double and float (the same values rounded), whose out-of-range narrow
 * conversions follow the same rule (SSE's 32-bit conversion, truncated). */
static u64 to_integer(unsigned kind, long double l) {
  if (kind >= 16) {
    float f = (float)l;
    switch (kind - 16) {
    case 0: return (u64)(long long)(signed char)f;
    case 1: return (unsigned char)f;
    case 2: return (u64)(long long)(short)f;
    case 3: return (unsigned short)f;
    case 4: return (u64)(long long)(int)f;
    case 5: return (unsigned)f;
    case 6: return (u64)(long long)f;
    default: return (u64)f;
    }
  }
  if (kind >= 8) {
    double d = (double)l;
    switch (kind - 8) {
    case 0: return (u64)(long long)(signed char)d;
    case 1: return (unsigned char)d;
    case 2: return (u64)(long long)(short)d;
    case 3: return (unsigned short)d;
    case 4: return (u64)(long long)(int)d;
    case 5: return (unsigned)d;
    case 6: return (u64)(long long)d;
    default: return (u64)d;
    }
  }
  long double x = l;
  switch (kind) {
  case 0: return (u64)(long long)(signed char)x;
  case 1: return (unsigned char)x;
  case 2: return (u64)(long long)(short)x;
  case 3: return (unsigned short)x;
  case 4: return (u64)(long long)(int)x;
  case 5: return (unsigned)x;
  case 6: return (u64)(long long)x;
  default: return (u64)x;
  }
}

#define NI 14
static volatile long long FROM[NI] = {0,
                                      1,
                                      -1,
                                      127,
                                      -128,
                                      255,
                                      65535,
                                      16777217,
                                      2147483647,
                                      -2147483647 - 1,
                                      9007199254740993LL,
                                      9223372036854775807LL,
                                      -9223372036854775807LL - 1,
                                      -3};

static U from_integer(unsigned kind, long long v) {
  switch (kind) {
  case 0: return lbits((long double)(signed char)v);
  case 1: return lbits((long double)(unsigned char)v);
  case 2: return lbits((long double)(short)v);
  case 3: return lbits((long double)(unsigned short)v);
  case 4: return lbits((long double)(int)v);
  case 5: return lbits((long double)(unsigned)v);
  case 6: return lbits((long double)v);
  default: return lbits((long double)(u64)v);
  }
}

/* The rounding functions, fabs, copysign, fmin, fmax, lrint, llround, powi on one value (and the next). */
#define UNARY 16
static U unary(unsigned k, unsigned i) {
  long double x = X(i), y = X((i + 5) % NX);
  switch (k) {
  case 0: return lbits(__builtin_fabsl(x));
  case 1: return lbits(__builtin_copysignl(x, y));
  case 2: return lbits(__builtin_floorl(x));
  case 3: return lbits(__builtin_ceill(x));
  case 4: return lbits(__builtin_truncl(x));
  case 5: return lbits(__builtin_roundl(x));
  case 6: return lbits(__builtin_rintl(x));
  case 7: return lbits(__builtin_nearbyintl(x));
  case 8: return lbits(__builtin_roundevenl(x));
  case 9: return lnan(lbits(x)) && lnan(lbits(y)) ? 1 : lbits(__builtin_fminl(x, y));
  case 10: return lnan(lbits(x)) && lnan(lbits(y)) ? 1 : lbits(__builtin_fmaxl(x, y));
  case 11: return lbits(-x);
  case 12: return !(__builtin_fabsl(x) < 0x1p63L) ? 0 : (u64)__builtin_llrintl(x); /* in range only */
  case 13: return !(__builtin_fabsl(x) < 0x1p63L) ? 0 : (u64)__builtin_llroundl(x);
  case 14: return lbits(__builtin_powil(x, 7));
  default: return lbits(__builtin_powil(x, -3));
  }
}

/* The fused multiply-add (fmal, glibc's natively): every pair of directed values, with a third as the
 * addend, and with the negated rounded product as the addend, which leaves the product's rounding error. A
 * NaN result of two NaNs, or of a NaN product negated (whose sign LLVM does not keep), is "a NaN". */
static U fused(unsigned i, unsigned j, int cancel) {
  long double a = X(i), b = X(j), c = cancel ? -(a * b) : X((i * 7 + j * 3) % NX);
  U bits = lbits(__builtin_fmal(a, b, c));
  int nans = lnan(lbits(a)) + lnan(lbits(b)) + lnan(lbits(c));
  return lnan(bits) && (nans >= 2 || (cancel && lnan(lbits(c)))) ? 1 : bits;
}

/* fmin and fmax on the cases glibc decides: zeros of either sign, NaNs against numbers and NaNs, equal
 * values. */
#define NM2 10
static const unsigned MINMAX[NM2][2] = {{0, 1}, {1, 0}, {16, 2}, {2, 16}, {15, 2}, {15, 16}, {16, 15},
                                        {15, 17}, {17, 15}, {5, 5}};

/* The sections, in order. */
#define ARITH (4 * NX * NX)
#define SQRT NX
#define FMA NT
#define COMPARE (NX * NX)
#define CLASS NX
#define NARROW (2 * (NN + NX))
#define WIDEN (2 * ND)
#define TO_INT (24 * NC)
#define FROM_INT (8 * NI)
#define INTRINSIC (UNARY * NX)
#define MINMAX_ (2 * NM2)
#define FUSED (2 * NX * NX)

static unsigned count(void) {
  return ARITH + SQRT + FMA + COMPARE + CLASS + NARROW + WIDEN + TO_INT + FROM_INT + INTRINSIC + MINMAX_ + FUSED;
}

static U value(unsigned i) {
  if (i < ARITH) {
    return arith(i / (NX * NX), i / NX % NX, i % NX);
  }
  i -= ARITH;
  if (i < SQRT) {
    return lbits(__builtin_sqrtl(X(i)));
  }
  i -= SQRT;
  if (i < FMA) {
    long double a = X(T[i][0]), b = X(T[i][1]), c = X(T[i][2]);
    return lbits(a * b + c);
  }
  i -= FMA;
  if (i < COMPARE) {
    long double a = X(i / NX), b = X(i % NX);
    return COMPARISONS(a, b);
  }
  i -= COMPARE;
  if (i < CLASS) {
    long double x = X(i);
    return CLASSES(x);
  }
  i -= CLASS;
  if (i < NARROW) {
    unsigned k = i % (NN + NX);
    long double x = k < NN ? ld(NE[k], NM[k]) : X(k - NN);
    return i < NN + NX ? dbits((double)x) : fbits((float)x);
  }
  i -= NARROW;
  if (i < WIDEN) {
    if (i < ND) {
      double d;
      __builtin_memcpy(&d, (const void *)&D[i], 8);
      return lbits(d);
    }
    float f;
    __builtin_memcpy(&f, (const void *)&F[i - ND], 4);
    return lbits(f);
  }
  i -= WIDEN;
  if (i < TO_INT) {
    return to_integer(i / NC, TO[i % NC]);
  }
  i -= TO_INT;
  if (i < FROM_INT) {
    return from_integer(i / NI, FROM[i % NI]);
  }
  i -= FROM_INT;
  if (i < INTRINSIC) {
    return unary(i / NX, i % NX);
  }
  i -= INTRINSIC;
  if (i < MINMAX_) {
    long double x = X(MINMAX[i % NM2][0]), y = X(MINMAX[i % NM2][1]);
    return lbits(i < NM2 ? __builtin_fminl(x, y) : __builtin_fmaxl(x, y));
  }
  i -= MINMAX_;
  return fused(i / NX % NX, i % NX, i >= NX * NX);
}

unsigned results(void) { return 2 * count(); }
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
