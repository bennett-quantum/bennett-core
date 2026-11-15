/* float_controlled.c: doubles under quantum control. Each function's comment states the state just
 * before its first measurement; floating.rs checks it, computing the doubles in Rust. */
#include "bennett.h"

static unsigned long long bits(double x) {
  unsigned long long r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}

/* (|0, 0> + |1, bits(x * 0.5 + 1.0)>)/sqrt(2) in (q, y), for x the double of bits `xbits`: an
 * assignment under a qubit branch. */
unsigned long long scaled(unsigned long long xbits) {
  _Bool q = 0;
  unsigned long long y = 0;
  double x;
  __builtin_memcpy(&x, &xbits, 8);
  bennett_h(&q);
  if (q)
    y = bits(x * 0.5 + 1.0);
  unsigned long long r = bennett_measure(&y);
  return r ^ bennett_measure(&q);
}

/* The sum over k < 16 of |k, bits(k / 3.0 + 0.1), k * 0.3 > 2.0>/4 in (q, y, big), where q holds k's
 * bits one per byte: a computation on doubles applied coherently, and a gate controlled by a comparison
 * of doubles. */
unsigned long long ratio(void) {
  _Bool q[4] = {0};
  for (int i = 0; i < 4; i++)
    bennett_h(&q[i]);
  unsigned char k = 0;
  for (int i = 0; i < 4; i++)
    k |= (unsigned char)(q[i] << i);
  unsigned long long y = bits((double)k / 3.0 + 0.1);
  _Bool big = 0;
  if ((double)k * 0.3 > 2.0)
    bennett_x(&big);
  unsigned long long r = bennett_measure(&y);
  r ^= bennett_measure(&big);
  return r ^ bennett_measure(q);
}

/* A rotation whose angle is computed on doubles, by a function the optimiser folds: RY(0.5) on a. */
static double half(double x) { return x * 0.5; }
unsigned long long folded(void) {
  _Bool a = 0;
  bennett_ry(&a, half(1.0));
  return bennett_measure(&a);
}
