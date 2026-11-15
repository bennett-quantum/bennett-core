/* float_programs.c: ordinary C on doubles and floats: constants, select and phi, loops, arrays and
 * structs, structs passed and returned by value (in registers, as a vector of two floats, in memory),
 * a variadic function summing doubles, function pointers with floating-point signatures, the
 * intrinsics (fabs, copysign, rounding, fmin, fmax, lrint, lround, powi, fma, sqrt), and vectors of
 * doubles. See float_values.c for how the results are checked. */

#include <stdarg.h>

typedef unsigned long long u64;
#define NOINLINE __attribute__((noinline))

static u64 bits(double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}
static u64 fbits(float x) {
  unsigned r;
  __builtin_memcpy(&r, &x, 4);
  return r;
}

#define NV 12
static volatile double V[NV] = {0.0, -0.0, 0.5, -0.5, 1.5, -2.5, 2.5, 3.7, -3.7, 1e300, -1e-310, 4503599627370495.5};

static const double table[5] = {1.0, 0.5, 0.25, 0.125, 0.0625};
static double accumulated = 1.0;

/* Loops, phi, constants. */
NOINLINE static double harmonic(int n) {
  double sum = 0.0;
  for (int k = 1; k <= n; k++) {
    sum += 1.0 / k;
  }
  return sum;
}

NOINLINE static double newton_sqrt(double x) {
  double r = x > 1.0 ? x : 1.0;
  for (int i = 0; i < 40; i++) {
    r = 0.5 * (r + x / r);
  }
  return r;
}

NOINLINE static float float_series(int n) {
  float sum = 0.0f, term = 1.0f;
  for (int k = 1; k <= n; k++) {
    term *= 0.5f;
    sum += term * (float)k;
  }
  return sum;
}

/* Arrays: an insertion sort, then a dot product. */
NOINLINE static double sorted_dot(void) {
  double a[8], b[8];
  for (int i = 0; i < 8; i++) {
    a[i] = V[(i * 5) % NV];
    b[i] = table[i % 5];
  }
  for (int i = 1; i < 8; i++) {
    double x = a[i];
    int j = i - 1;
    while (j >= 0 && a[j] > x) {
      a[j + 1] = a[j];
      j--;
    }
    a[j + 1] = x;
  }
  double dot = 0.0;
  for (int i = 0; i < 8; i++) {
    if (a[i] < 1e299 && a[i] > -1e299) {
      dot += a[i] * b[i];
    }
  }
  return dot + a[0] * 1e-300;
}

/* Structs by value: two doubles in registers, three in memory, two floats as a vector, mixed. */
struct complex {
  double re, im;
};
struct triple {
  double x, y, z;
};
struct pair {
  float a, b;
};
struct mixed {
  double d;
  long n;
};

NOINLINE static struct complex cmul(struct complex p, struct complex q) {
  struct complex r = {p.re * q.re - p.im * q.im, p.re * q.im + p.im * q.re};
  return r;
}
NOINLINE static struct triple cross(struct triple u, struct triple v) {
  struct triple r = {u.y * v.z - u.z * v.y, u.z * v.x - u.x * v.z, u.x * v.y - u.y * v.x};
  return r;
}
NOINLINE static struct pair swap_scale(struct pair p, float s) {
  struct pair r = {p.b * s, p.a / s};
  return r;
}
NOINLINE static struct mixed scale(struct mixed m) {
  struct mixed r = {m.d * (double)m.n, m.n + (long)m.d};
  return r;
}

/* A variadic function of our own summing doubles (floats arrive as doubles). */
NOINLINE static double sum(int n, ...) {
  va_list list;
  va_start(list, n);
  double total = 0.0;
  for (int i = 0; i < n; i++) {
    total += va_arg(list, double);
  }
  va_end(list);
  return total;
}

/* Function pointers with floating-point signatures. */
static double add(double a, double b) { return a + b; }
static double sub(double a, double b) { return a - b; }
static double mul(double a, double b) { return a * b; }
static double divide(double a, double b) { return a / b; }
static double (*volatile const operations[4])(double, double) = {add, sub, mul, divide};
static float halve(float x) { return x * 0.5f; }
static float (*volatile const halving)(float) = halve;

/* Vectors of doubles (GCC's vector extensions), element by element. */
typedef double v2 __attribute__((vector_size(16)));
typedef long long l2 __attribute__((vector_size(16)));

NOINLINE static u64 vectors(double x, double y) {
  v2 a = {x, y}, b = {y, 0.25};
  v2 c = a * b + a;
  l2 less = a < b;
  return bits(c[0]) ^ bits(c[1]) * 3 ^ (u64)less[0] * 5 ^ (u64)less[1] * 7;
}

/* A long double is only moved (computing on it is not handled yet). */
struct holder {
  long double value;
  int tag;
};
NOINLINE static struct holder move(struct holder h) {
  struct holder r = h;
  r.tag++;
  return r;
}

#define INTRINSICS 17
static u64 intrinsic(unsigned k, double x, double y) {
  float f = (float)x;
  switch (k) {
  case 0: return bits(__builtin_fabs(x)) ^ fbits(__builtin_fabsf(f)) << 1;
  case 1: return bits(__builtin_copysign(x, y)) ^ fbits(__builtin_copysignf(f, (float)y)) << 1;
  case 2: return bits(__builtin_floor(x)) ^ fbits(__builtin_floorf(f)) << 1;
  case 3: return bits(__builtin_ceil(x)) ^ fbits(__builtin_ceilf(f)) << 1;
  case 4: return bits(__builtin_trunc(x)) ^ fbits(__builtin_truncf(f)) << 1;
  case 5: return bits(__builtin_round(x)) ^ fbits(__builtin_roundf(f)) << 1;
  case 6: return bits(__builtin_rint(x)) ^ fbits(__builtin_rintf(f)) << 1;
  case 7: return bits(__builtin_nearbyint(x)) ^ fbits(__builtin_nearbyintf(f)) << 1;
  case 8: return bits(__builtin_roundeven(x)) ^ fbits(__builtin_roundevenf(f)) << 1;
  case 9: /* the sign of a zero result is unspecified */
    return x == y ? 0 : bits(__builtin_fmin(x, y)) ^ fbits(__builtin_fminf(f, (float)y)) << 1;
  case 10: return x == y ? 0 : bits(__builtin_fmax(x, y)) ^ fbits(__builtin_fmaxf(f, (float)y)) << 1;
  case 11: /* in range only: out of range is unspecified */
    return x > -1e18 && x < 1e18 ? (u64)__builtin_lrint(x) : 0;
  case 12: return x > -1e18 && x < 1e18 ? (u64)__builtin_llround(x) : 0;
  case 13: return f > -1e9f && f < 1e9f ? (u64)__builtin_lroundf(f) ^ (u64)__builtin_llrintf(f) << 32 : 0;
  case 14: return bits(__builtin_powi(x, 3)) ^ bits(__builtin_powi(y, -2));
  case 15: return fbits(__builtin_powif(f, 5)) ^ bits(__builtin_powi(x, 0));
  default: return bits(__builtin_fma(x, y, 0.25)) ^ fbits(__builtin_sqrtf(f < 0 ? -f : f)) << 1;
  }
}

#define PROGRAMS 13
static u64 program(unsigned k) {
  volatile int n = 100;
  switch (k) {
  case 0: return bits(harmonic(n));
  case 1: return bits(newton_sqrt(V[9] * 1e-290)) ^ bits(newton_sqrt(2.0)) * 3;
  case 2: return fbits(float_series(n / 4));
  case 3: return bits(sorted_dot());
  case 4: {
    struct complex p = {V[4], V[5]}, q = {V[7], V[2]};
    struct complex r = cmul(p, q);
    return bits(r.re) ^ bits(r.im) * 3;
  }
  case 5: {
    struct triple u = {V[4], V[5], V[6]}, v = {V[7], V[8], V[2]};
    struct triple r = cross(u, v);
    return bits(r.x) ^ bits(r.y) * 3 ^ bits(r.z) * 5;
  }
  case 6: {
    struct pair p = {(float)V[4], (float)V[7]};
    struct pair r = swap_scale(p, 3.0f);
    return fbits(r.a) ^ fbits(r.b) << 32;
  }
  case 7: {
    struct mixed m = {V[7], -3};
    struct mixed r = scale(m);
    return bits(r.d) ^ (u64)r.n * 7;
  }
  case 8: return bits(sum(10, V[0], V[1], V[2], V[3], V[4], V[5], V[6], V[7], (double)(float)V[8], V[11]));
  case 9: {
    u64 r = 0;
    for (int i = 0; i < 4; i++) {
      r = r * 31 + bits(operations[i](V[7], V[4]));
    }
    return r ^ fbits(halving((float)V[5]));
  }
  case 10: return vectors(V[4], V[7]) ^ vectors(V[8], V[3]) * 11;
  case 11: {
    struct holder h = {1.0L, 4};
    struct holder r = move(h);
    u64 low;
    __builtin_memcpy(&low, &r.value, 8);
    return low ^ (u64)r.tag;
  }
  default: {
    accumulated = accumulated * 1.5 + table[n % 5];
    double x = V[n % NV];
    return bits(x > 0.0 ? accumulated : -accumulated) ^ bits(x != x ? 1.0 : 2.0);
  }
  }
}

unsigned results(void) { return PROGRAMS + INTRINSICS * NV * 2; }

u64 result(unsigned i) {
  if (i < PROGRAMS) {
    return program(i);
  }
  i -= PROGRAMS;
  unsigned k = i / (NV * 2), j = i % (NV * 2);
  return intrinsic(k, V[j % NV], V[(j / 2 + 3) % NV]);
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
