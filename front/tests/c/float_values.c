/* float_values.c: floating-point arithmetic, square roots, fused and unfused multiply-adds and
 * conversions on directed values (signed zeros, subnormals, infinities, NaNs, rounding ties, overflow).
 * result(i) is the bit pattern of the i-th result, results() how many there are; built with -DNATIVE,
 * main prints them all. The tests compare the device's with the native build's, one by one.
 *
 * Where both operands are NaNs, which one's payload comes out is the compiler's choice (it may swap the
 * operands of an addition), so such a result is compared as "a NaN" only. */

typedef unsigned long long u64;
typedef unsigned int u32;

static u64 dbits(double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}
static u64 fbits(float x) {
  u32 r;
  __builtin_memcpy(&r, &x, 4);
  return r;
}
static double dof(u64 bits) {
  double r;
  __builtin_memcpy(&r, &bits, 8);
  return r;
}
static float fof(u32 bits) {
  float r;
  __builtin_memcpy(&r, &bits, 4);
  return r;
}

#define ND 18
static volatile u64 D[ND] = {
    0x0000000000000000, /* 0 */
    0x8000000000000000, /* -0 */
    0x3FF0000000000000, /* 1 */
    0xBFF0000000000000, /* -1 */
    0x3FB999999999999A, /* 0.1 */
    0x4008000000000000, /* 3 */
    0x3FF0000000000001, /* 1 + 2^-52 */
    0x4340000000000000, /* 2^53 */
    0xC33FFFFFFFFFFFFF, /* -(2^53 - 1) */
    0x0000000000000001, /* the least subnormal */
    0x000FFFFFFFFFFFFF, /* the greatest subnormal */
    0x0010000000000000, /* the least normal */
    0x7FEFFFFFFFFFFFFF, /* the greatest finite */
    0x7FF0000000000000, /* inf */
    0xFFF0000000000000, /* -inf */
    0x7FF8000000000123, /* a quiet NaN with a payload */
    0x7FF0000000000456, /* a signalling NaN */
    0xFFF8000000000000, /* x86's default NaN */
};

#define NF 18
static volatile u32 F[NF] = {
    0x00000000, 0x80000000, 0x3F800000, 0xBF800000, 0x3DCCCCCD, 0x40400000, 0x3F800001, 0x4B800000, 0xCB7FFFFF,
    0x00000001, 0x007FFFFF, 0x00800000, 0x7F7FFFFF, 0x7F800000, 0xFF800000, 0x7FC00123, 0x7F800456, 0xFFC00000,
};

static int dnan(u64 b) { return (b & 0x7FFFFFFFFFFFFFFF) > 0x7FF0000000000000; }
static int fnan(u32 b) { return (b & 0x7FFFFFFF) > 0x7F800000; }

static u64 darith(unsigned op, u64 x, u64 y) {
  double a = dof(x), b = dof(y), r;
  switch (op) {
  case 0: r = a + b; break;
  case 1: r = a - b; break;
  case 2: r = a * b; break;
  default: r = a / b; break;
  }
  u64 bits = dbits(r);
  return dnan(x) && dnan(y) && dnan(bits) ? 0x7FF8000000000000 : bits;
}

static u64 farith(unsigned op, u32 x, u32 y) {
  float a = fof(x), b = fof(y), r;
  switch (op) {
  case 0: r = a + b; break;
  case 1: r = a - b; break;
  case 2: r = a * b; break;
  default: r = a / b; break;
  }
  u32 bits = (u32)fbits(r);
  return fnan(x) && fnan(y) && fnan(bits) ? 0x7FC00000 : bits;
}

/* Triples where a fused multiply-add and the unfused a * b + c differ, or are special. */
#define NT 10
static volatile u64 TD[NT][3] = {
    {0x3FB999999999999A, 0x4024000000000000, 0xBFF0000000000000}, /* 0.1 * 10 - 1 */
    {0x3FF0000000000001, 0x3FEFFFFFFFFFFFFE, 0xBFF0000000000000}, /* (1 + e)(1 - e) - 1 */
    {0x7FEFFFFFFFFFFFFF, 0x4000000000000000, 0xFFEFFFFFFFFFFFFF}, /* MAX * 2 - MAX */
    {0x7FF0000000000000, 0x0000000000000000, 0x3FF0000000000000}, /* inf * 0 + 1 */
    {0x3FF0000000000000, 0x3FF0000000000000, 0x7FF8000000000123}, /* 1 * 1 + NaN */
    {0x8000000000000000, 0x0000000000000000, 0x8000000000000000}, /* -0 * 0 + -0 */
    {0x0000000000000001, 0x3FE0000000000000, 0x0000000000000000}, /* subnormal * 0.5 + 0 */
    {0x4008000000000000, 0x3FD5555555555555, 0xBFF0000000000000}, /* 3 * (1/3) - 1 */
    {0x3FF8000000000000, 0x3FF8000000000000, 0xC002000000000000}, /* 1.5 * 1.5 - 2.25 */
    {0x41A0000000000001, 0x41A0000000000001, 0xC350000000000000}, /* near 2^27 squared - 2^54 */
};
static volatile u32 TF[NT][3] = {
    {0x3DCCCCCD, 0x41200000, 0xBF800000}, {0x3F800001, 0x3F7FFFFE, 0xBF800000}, {0x7F7FFFFF, 0x40000000, 0xFF7FFFFF},
    {0x7F800000, 0x00000000, 0x3F800000}, {0x3F800000, 0x3F800000, 0x7FC00123}, {0x80000000, 0x00000000, 0x80000000},
    {0x00000001, 0x3F000000, 0x00000000}, {0x40400000, 0x3EAAAAAB, 0xBF800000}, {0x3FC00000, 0x3FC00000, 0xC0100000},
    {0x45800001, 0x45800001, 0xCB800000},
};

/* Conversions to integers, from values in range of each type (out of range is undefined in C). */
#define NC 8
static volatile double TOI8[NC] = {0.0, -0.0, 0.9, -0.9, 1.5, -1.5, 127.99, -128.99};
static volatile double TOU8[NC] = {0.0, -0.0, 0.9, -0.9, 1.5, 200.5, 255.99, 3.0};
static volatile double TOI16[NC] = {0.0, -0.5, 7.75, -7.75, 32767.9, -32768.9, 1e-300, -1e-300};
static volatile double TOU16[NC] = {0.0, -0.5, 7.75, 1000.25, 65535.9, 32768.0, 1e-300, 0.999};
static volatile double TOI32[NC] = {0.0, -0.9, 2147483647.9, -2147483648.9, 1e9, -1e9, 0.5, -123456.789};
static volatile double TOU32[NC] = {0.0, -0.9, 4294967295.9, 2147483648.0, 3e9, 1e9, 0.5, 123456.789};
static volatile double TOI64[NC] = {0.0, -0.9, 9.2233720368547748e18, -9.2233720368547758e18, 9007199254740993.0,
                                    -4503599627370497.5, 1e18, -1e-320};
static volatile double TOU64[NC] = {0.0, -0.9, 1.8446744073709550e19, 9.2233720368547758e18, 9.2233720368547748e18,
                                    4503599627370497.5, 1e18, 1e-320};

/* Conversions from integers: values whose rounding is interesting. */
#define NI 12
static volatile long long FROM[NI] = {0,
                                      1,
                                      -1,
                                      127,
                                      -128,
                                      16777217,
                                      -16777217,
                                      2147483647,
                                      -2147483647 - 1,
                                      9007199254740993LL,
                                      9223372036854775807LL,
                                      -9223372036854775807LL - 1};

/* From double, then from float: half the value, which stays in range once rounded to float. */
static u64 to_integer(unsigned kind, double d) {
  float f = (float)(d * 0.5);
  switch (kind) {
  case 0: return (u64)(long long)(signed char)d;
  case 1: return (unsigned char)d;
  case 2: return (u64)(long long)(short)d;
  case 3: return (unsigned short)d;
  case 4: return (u64)(long long)(int)d;
  case 5: return (unsigned)d;
  case 6: return (u64)(long long)d;
  case 7: return (u64)d;
  case 8: return (u64)(long long)(signed char)f;
  case 9: return (unsigned char)f;
  case 10: return (u64)(long long)(short)f;
  case 11: return (unsigned short)f;
  case 12: return (u64)(long long)(int)f;
  case 13: return (unsigned)f;
  case 14: return (u64)(long long)f;
  default: return (u64)f;
  }
}

static volatile double *const TO[8] = {TOI8, TOU8, TOI16, TOU16, TOI32, TOU32, TOI64, TOU64};

static u64 from_integer(unsigned kind, long long v) {
  switch (kind) {
  case 0: return dbits((double)(signed char)v);
  case 1: return dbits((double)(unsigned char)v);
  case 2: return dbits((double)(short)v);
  case 3: return dbits((double)(unsigned short)v);
  case 4: return dbits((double)(int)v);
  case 5: return dbits((double)(unsigned)v);
  case 6: return dbits((double)v);
  case 7: return dbits((double)(u64)v);
  case 8: return fbits((float)(signed char)v);
  case 9: return fbits((float)(unsigned char)v);
  case 10: return fbits((float)(short)v);
  case 11: return fbits((float)(unsigned short)v);
  case 12: return fbits((float)(int)v);
  case 13: return fbits((float)(unsigned)v);
  case 14: return fbits((float)v);
  default: return fbits((float)(u64)v);
  }
}

/* The sections, in order. */
#define ARITH_D (4 * ND * ND)
#define ARITH_F (4 * NF * NF)
#define SQRT (ND + NF)
#define FMA (4 * NT)
#define WIDEN (ND + NF)
#define TO_INT (16 * NC)
#define FROM_INT (16 * NI)

unsigned results(void) { return ARITH_D + ARITH_F + SQRT + FMA + WIDEN + TO_INT + FROM_INT; }

u64 result(unsigned i) {
  if (i < ARITH_D) {
    return darith(i / (ND * ND), D[i / ND % ND], D[i % ND]);
  }
  i -= ARITH_D;
  if (i < ARITH_F) {
    return farith(i / (NF * NF), F[i / NF % NF], F[i % NF]);
  }
  i -= ARITH_F;
  if (i < SQRT) {
    return i < ND ? dbits(__builtin_sqrt(dof(D[i]))) : fbits(__builtin_sqrtf(fof(F[i - ND])));
  }
  i -= SQRT;
  if (i < FMA) {
    unsigned t = i % NT;
    double a = dof(TD[t][0]), b = dof(TD[t][1]), c = dof(TD[t][2]);
    float x = fof(TF[t][0]), y = fof(TF[t][1]), z = fof(TF[t][2]);
    switch (i / NT) {
    case 0: return dbits(__builtin_fma(a, b, c));
    case 1: return dbits(a * b + c); /* llvm.fmuladd: unfused, as natively */
    case 2: return fbits(__builtin_fmaf(x, y, z));
    default: return fbits(x * y + z);
    }
  }
  i -= FMA;
  if (i < WIDEN) {
    if (i < ND) {
      u64 bits = fbits((float)dof(D[i]));
      return fnan((u32)bits) ? bits | 0x100000000 : bits;
    }
    return dbits((double)fof(F[i - ND]));
  }
  i -= WIDEN;
  if (i < TO_INT) {
    return to_integer(i / NC, TO[i / NC % 8][i % NC]);
  }
  i -= TO_INT;
  return from_integer(i / NI, FROM[i % NI]);
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
