/* float_long_double_programs.c: ordinary C on long double: constants, select and phi, loops, arrays,
 * globals, a struct holding a long double passed and returned by value, a variadic function reading long
 * doubles (with doubles and integers between them) with va_arg, long double parameters and results of
 * functions and function pointers, mixed float, double and long double expressions. Each result is a
 * 64-bit hash of what a program computed. See float_values.c for how the results are checked. */

#include <stdarg.h>

typedef unsigned long long u64;
#define NOINLINE __attribute__((noinline))

static u64 lbits(long double x) {
  u64 m = 0;
  unsigned short e = 0;
  __builtin_memcpy(&m, &x, 8);
  __builtin_memcpy(&e, (const char *)&x + 8, 2);
  return m ^ (u64)e * 0x9E3779B97F4A7C15;
}

#define NV 10
static volatile long double V[NV] = {
    0.0L, -0.0L, 0.5L, -2.5L, 3.7L, 1e300L, -1e-4940L, 1e4000L, 0x1.fffffffffffffffep16383L, 18446744073709551615.0L,
};

static const long double table[4] = {1.0L, 0.1L, 1e-30L, 3.0L};
static long double accumulated = 1.0L;

/* Loops, phi, constants. */
NOINLINE static long double harmonic(int n) {
  long double sum = 0.0L;
  for (int k = 1; k <= n; k++) {
    sum += 1.0L / k;
  }
  return sum;
}

NOINLINE static long double newton_sqrt(long double x) {
  long double r = x > 1.0L ? x : 1.0L;
  for (int i = 0; i < 70; i++) {
    r = 0.5L * (r + x / r);
  }
  return r;
}

/* Arrays: an insertion sort, then a dot product with a table of constants. */
NOINLINE static long double sorted_dot(void) {
  long double a[8];
  for (int i = 0; i < 8; i++) {
    a[i] = V[(i * 3) % NV] * (i + 1);
  }
  for (int i = 1; i < 8; i++) {
    long double t = a[i];
    int j = i - 1;
    while (j >= 0 && a[j] > t) {
      a[j + 1] = a[j];
      j--;
    }
    a[j + 1] = t;
  }
  long double dot = 0.0L;
  for (int i = 0; i < 8; i++) {
    dot += (a[i] < 1e100L && a[i] > -1e100L ? a[i] : 1.0L) * table[i % 4];
  }
  return dot;
}

/* A struct holding a long double, passed and returned by value (in memory). */
struct holder {
  long double value;
  int tag;
};
NOINLINE static struct holder scale(struct holder h, long double by) {
  struct holder r = {h.value * by, h.tag + 1};
  return r;
}

/* A struct of a long double and a double, and one of two long doubles. */
struct mixed {
  double d;
  long double x;
  float f;
};
NOINLINE static struct mixed blend(struct mixed m) {
  struct mixed r = {(double)m.x, m.d + m.f, (float)(m.x * m.d)};
  return r;
}

/* A variadic function: n long doubles, each followed by a double and an int. */
NOINLINE static long double sum(int n, ...) {
  va_list args;
  va_start(args, n);
  long double total = 0.0L;
  for (int i = 0; i < n; i++) {
    long double x = va_arg(args, long double);
    double d = va_arg(args, double);
    int k = va_arg(args, int);
    total = total * 3 + x + d * k;
  }
  va_end(args);
  return total;
}

/* Function pointers with long double signatures. */
static long double add(long double a, long double b) { return a + b; }
static long double sub(long double a, long double b) { return a - b; }
static long double mul(long double a, long double b) { return a * b; }
static long double divide(long double a, long double b) { return a / b; }
static long double (*volatile const operations[4])(long double, long double) = {add, sub, mul, divide};
static long double widen(double d, float f) { return (long double)d * f; }
static long double (*volatile const widening)(double, float) = widen;

#define PROGRAMS 12
static u64 program(unsigned k) {
  volatile int n = 100;
  switch (k) {
  case 0: return lbits(harmonic(n));
  case 1: return lbits(newton_sqrt(V[5])) ^ lbits(newton_sqrt(2.0L)) * 3;
  case 2: return lbits(sorted_dot());
  case 3: {
    struct holder h = {V[4], 4};
    struct holder r = scale(h, V[3]);
    return lbits(r.value) ^ (u64)r.tag * 7;
  }
  case 4: {
    struct mixed m = {0.1, V[4], 2.5f};
    struct mixed r = blend(m);
    return lbits(r.x) ^ lbits(r.d) * 3 ^ lbits(r.f) * 5;
  }
  case 5: return lbits(sum(4, V[2], 0.25, 3, V[3], -1.5, -2, V[9], 1e10, 7, V[4], 0.1, 1));
  case 6: {
    u64 r = 0;
    for (int i = 0; i < 4; i++) {
      r = r * 31 + lbits(operations[i](V[4], V[3]));
    }
    return r ^ lbits(widening(0.1, 0.3f));
  }
  case 7: {
    /* select and phi on long double, on a value the program cannot know */
    long double x = V[n % NV], y = V[(n + 3) % NV];
    long double m = x > y ? x : y;
    for (int i = 0; i < 3; i++) {
      m = m < 1.0L ? m + 1.0L : m * 0.5L;
    }
    return lbits(m);
  }
  case 8: {
    accumulated = accumulated * 1.5L + table[n % 4];
    accumulated = accumulated * 1.5L + table[(n + 1) % 4];
    return lbits(accumulated);
  }
  case 9: {
    /* float, double and long double in one expression */
    float f = 0.1f;
    double d = 0.1;
    long double x = V[4];
    return lbits(f * x + d) ^ lbits((double)(x * d) + f) * 3 ^ lbits((float)x / f) * 5;
  }
  case 10: {
    /* overflow, underflow and the largest value */
    long double big = V[8], tiny = V[6];
    return lbits(big + big) ^ lbits(tiny * 0.5L) * 3 ^ lbits(big * tiny) * 5 ^ lbits(V[7] / V[5]) * 7;
  }
  default: {
    /* integers through long double and back */
    u64 r = 0;
    for (int i = -3; i < 3; i++) {
      long double x = (long double)(u64)(i * 0x2AAAAAAAAAAAAAAALL) / 3;
      r = r * 31 + (u64)(long long)x + (unsigned)(x * 0x1p-33L);
    }
    return r;
  }
  }
}

unsigned results(void) { return PROGRAMS; }

u64 result(unsigned i) { return program(i); }

#ifdef NATIVE
int printf(const char *, ...);
int main(void) {
  for (unsigned i = 0; i < results(); i++) {
    printf("%llu\n", result(i));
  }
  return 0;
}
#endif
