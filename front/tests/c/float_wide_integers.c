/* float_wide_integers.c: conversions between float, double, long double and 128-bit integers (and a
 * _BitInt(100), which goes through the same routines), on directed values: ties and sticky bits at each
 * format's rounding position, the edges of the 64- and 128-bit ranges, and out of range (undefined in C,
 * compared with what the native program's libgcc routines give; a _BitInt(100) in range only). result(i) is the bit pattern of the i-th
 * result, results() how many there are; a 128-bit result is two, its high 64 bits and then its low 64 bits.
 * Built with -DNATIVE, main prints them all. */

typedef unsigned long long u64;
typedef unsigned __int128 U;
typedef __int128 S;

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

/* To integers: long double values, also taken as double and float. */
#define NT 24
static volatile long double TO[NT] = {
    0.0L,          -0.0L,          0.75L,        -0.75L,        -1.0L,         1e10L,
    -1e20L,        0x1p63L,        0x1p64L,      -0x1p64L,      0x1.8p100L,    -0x1.fffffffffffffffep126L,
    0x1p127L,      -0x1p127L,      0x1.8p127L,   0x1.fffffffffffffffep127L, 0x1p128L, -0x1p128L,
    1e300L,        -1e4000L,       1.0L / 0.0L,  -1.0L / 0.0L,  0.0L / 0.0L,   -(0.0L / 0.0L),
};

static U to_integer(unsigned kind, long double l) {
  double d = (double)l;
  float f = (float)l;
  switch (kind) {
  case 0: return (U)(S)l;
  case 1: return (U)l;
  case 2: return (U)(S)d;
  case 3: return (U)d;
  case 4: return (U)(S)f;
  case 5: return (U)f;
  /* Out of range of a _BitInt(100) the result is poison in LLVM, and the native build keeps whatever bits
   * are left: in range only. */
  case 6: return d > -0x1p99 && d < 0x1p99 ? (U)(S)(_BitInt(100))d : 0;
  default: return l > -1.0L && l < 0x1p100L ? (U)(unsigned _BitInt(100))l : 0;
  }
}

/* From integers: ties at the rounding position of float (24 bits), double (53) and long double (64), with
 * a sticky bit or one below, the edges of the ranges. */
#define NF 20
static volatile u64 HI[NF] = {
    0,
    0,
    0,
    0x8000000000000000,
    0x8000000000000000,
    0xFFFFFFFFFFFFFFFF,
    0x7FFFFFFFFFFFFFFF,
    0x0000010000000000,
    0x0000010000000000,
    0x0000010000000000,
    0x0000000000000001,
    0x0000000000000001,
    0x0000000000000001,
    0x0000000000000001,
    0xFFFFFFFFFFFFFFFF,
    0x0000000000000000,
    0xFFFFFF8000000000,
    0x0123456789ABCDEF,
    0x8000000000000000,
    0xFFFFFFFFFFFFFFFE,
};
static volatile u64 LO[NF] = {
    0,
    1,
    0xFFFFFFFFFFFFFFFF,
    0,
    1,
    0xFFFFFFFFFFFFFFFF,
    0xFFFFFFFFFFFFFFFF,
    0x0000008000000000, /* 2^104 + 2^103 - 2^63 ...: float's tie region */
    0x0000000000000001,
    0x8000000000000000,
    0x8000000000000000, /* 2^64 + 2^63: long double's tie at 65 bits, to even */
    0x8000000000000001, /* just above: up */
    0x7FFFFFFFFFFFFFFF, /* just below: down */
    0x0000000000000800,
    0x0000000000000000,
    0x0020000000000001, /* 2^53 + 1: double's tie below 2^64 */
    0x0000000000000000,
    0xFEDCBA9876543210,
    0x0000000000000001,
    0x0000000000000400,
};

static U from_integer(unsigned kind, U v) {
  switch (kind) {
  case 0: return fbits((float)(S)v);
  case 1: return fbits((float)v);
  case 2: return dbits((double)(S)v);
  case 3: return dbits((double)v);
  case 4: return lbits((long double)(S)v);
  case 5: return lbits((long double)v);
  case 6: return dbits((double)(_BitInt(100))(S)v);
  default: return lbits((long double)(unsigned _BitInt(100))v);
  }
}

#define TO_INT (8 * NT)
#define FROM_INT (8 * NF)

static U value(unsigned i) {
  if (i < TO_INT) {
    return to_integer(i / NT, TO[i % NT]);
  }
  i -= TO_INT;
  U v = (U)HI[i % NF] << 64 | LO[i % NF];
  return from_integer(i / NF, v);
}

unsigned results(void) { return 2 * (TO_INT + FROM_INT); }
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
