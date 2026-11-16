/* complex_noncanonical_x87.c: libgcc's long-double helpers on noncanonical x87 operands written through raw
 * memory, one witness per class (found by the independent libgcc testing pass): an unnormal, an unnormal
 * zero, a pseudo-denormal, a pseudo-infinity and a pseudo-NaN. x87 hardware refuses all but the
 * pseudo-denormal as invalid operands (a default NaN) and reads the pseudo-denormal at the smallest normal
 * exponent. The device lowers long double over SoftFloat without validating encodings, so every witness
 * differs from native in a non-NaN way: a finite or infinite value for a NaN, one ulp (the pseudo-denormal
 * compares below the smallest normal and libgcc's division takes its other branch), a zero's sign.
 * result(i): word i of the results, real then imaginary, each as (sign and exponent, significand).
 * Natively (-DNATIVE) it prints them. */
typedef unsigned long long u64;
typedef unsigned short u16;
long double _Complex __mulxc3(long double, long double, long double, long double);
long double _Complex __divxc3(long double, long double, long double, long double);

#define NW 5
/* Operands as (sign and exponent, significand); the helper: 0 multiply, 1 divide. */
static volatile u16 E[NW][4] = {{0x3fe0, 0, 0xc000, 0}, {0, 0, 0, 0x70e0}, {0x3fff, 0x8000, 0x0000, 0x0001},
                                {0xffff, 0, 0xc000, 0xc000}, {0x8000, 0x8000, 0x7fff, 0x7fff}};
static volatile u64 M[NW][4] = {{0x0086686f0f664c3full, 0, 0xc000000000000000ull, 0},
                                {0, 0, 0, 0},
                                {0x8000000000000000ull, 0, 0x8b3498f0b293067bull, 0x8000000000000000ull},
                                {0, 0, 0xc000000000000000ull, 0xc000000000000000ull},
                                {0, 0, 0x8000000000000000ull, 0x10e8662f429dc740ull}};
static const int DIVIDE[NW] = {0, 0, 1, 0, 1};

static long double x_of(u16 e, u64 m) {
  long double x = 0;
  __builtin_memcpy(&x, &m, 8);
  __builtin_memcpy((char *)&x + 8, &e, 2);
  return x;
}

int results(void) { return NW * 4; }

u64 result(int i) {
  int w = i / 4;
  long double v[4];
  for (int j = 0; j < 4; j++) v[j] = x_of(E[w][j], M[w][j]);
  long double _Complex z = DIVIDE[w] ? __divxc3(v[0], v[1], v[2], v[3]) : __mulxc3(v[0], v[1], v[2], v[3]);
  long double part = i % 4 < 2 ? __real__ z : __imag__ z;
  u64 m = 0;
  u16 e = 0;
  __builtin_memcpy(&m, &part, 8);
  __builtin_memcpy(&e, (char *)&part + 8, 2);
  return i % 2 ? m : e;
}

#ifdef NATIVE
int printf(const char *, ...);
int main(void) {
  for (int i = 0; i < results(); i++) printf("%llu\n", result(i));
  return 0;
}
#endif
