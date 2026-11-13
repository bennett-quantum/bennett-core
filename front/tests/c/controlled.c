/* controlled.c: gates under qubit control, written as ordinary C. Each function's comment states the
 * state just before its first measurement; the tests check that state, then the measurements. */
#include "bennett.h"

/* (|00> + |11>)/sqrt(2) in (a, b): a CNOT. */
unsigned long long bell(void) {
  _Bool a = 0, b = 0;
  bennett_h(&a);
  if (a)
    bennett_x(&b);
  unsigned long long r = bennett_measure(&a);
  return r | bennett_measure(&b) << 1;
}

/* cos(1/2) |01> + sin(1/2) |10> in (a, b): a CNOT controlled by a == 0, after RY(1) on a (not a
 * Hadamard, whose symmetric state would hide a control of the wrong polarity). */
unsigned long long anti(void) {
  _Bool a = 0, b = 0;
  bennett_ry(&a, 1.0);
  if (!a)
    bennett_x(&b);
  unsigned long long r = bennett_measure(&a);
  return r | bennett_measure(&b) << 1;
}

/* (|000> + |111>)/sqrt(2) in (a, b, c). */
unsigned long long ghz(void) {
  _Bool a = 0, b = 0, c = 0;
  bennett_h(&a);
  if (a)
    bennett_x(&b);
  if (b)
    bennett_x(&c);
  unsigned long long r = bennett_measure(&a);
  r |= bennett_measure(&b) << 1;
  return r | bennett_measure(&c) << 2;
}

/* The basis state (a, b, c ^ (a & b)) for in = a << 2 | b << 1 | c: a Toffoli. */
unsigned long long toffoli(unsigned char in) {
  _Bool a = 0, b = 0, c = 0;
  if (in & 4)
    bennett_x(&a);
  if (in & 2)
    bennett_x(&b);
  if (in & 1)
    bennett_x(&c);
  if (a && b)
    bennett_x(&c);
  unsigned long long r = bennett_measure(&a);
  r |= bennett_measure(&b) << 1;
  return r | bennett_measure(&c) << 2;
}

/* A controlled Z seen by interference: H, CZ with b = flip, H leaves a = flip, b = flip. With `negated`
 * the control is a == 0, which gives the same result up to a global phase. `negated` is an ordinary
 * value, so the condition is computed into a qubit of its own. */
unsigned long long phase(unsigned char flip, unsigned char negated) {
  _Bool a = 0, b = 0;
  if (flip)
    bennett_x(&b);
  bennett_h(&a);
  if (negated ? !a : a)
    bennett_z(&b);
  bennett_h(&a);
  unsigned long long r = bennett_measure(&a);
  return r | bennett_measure(&b) << 1;
}

/* A rotation under a qubit: a in |+>, then RY(theta) on b where a is 1. */
unsigned long long rotation(void) {
  _Bool a = 0, b = 0;
  bennett_h(&a);
  if (a)
    bennett_ry(&b, 1.25);
  unsigned long long r = bennett_measure(&a);
  return r | bennett_measure(&b) << 1;
}

static int f(unsigned p, unsigned q) { return (int)((p * 5 + q * 3) & 7); }

/* A gate under a computed condition: p and q in uniform superposition over 0..3, then c = [f(p, q) == 3]
 * on every branch. The condition is computed into a qubit of its own and uncomputed. */
unsigned long long computed(void) {
  unsigned char p = 0, q = 0;
  _Bool c = 0;
  bennett_h(bennett_bit(&p, 0));
  bennett_h(bennett_bit(&p, 1));
  bennett_h(bennett_bit(&q, 0));
  bennett_h(bennett_bit(&q, 1));
  if (f(p, q) == 3)
    bennett_x(&c);
  unsigned long long r = bennett_measure(&p);
  r |= bennett_measure(&q) << 8;
  return r | bennett_measure(&c) << 16;
}

/* Gates and stores under one condition, and under a nested one. Over p in 0..3: odd p gives d = p + 1
 * (an in-place update under the condition) and c = [p == 3]; even p gives d = 7. */
unsigned long long block(void) {
  unsigned char p = 0, d = 0;
  _Bool c = 0;
  bennett_h(bennett_bit(&p, 0));
  bennett_h(bennett_bit(&p, 1));
  if (p & 1) {
    d ^= p;
    if (p & 2)
      bennett_x(&c);
    d += 1;
  } else {
    d ^= 7;
  }
  unsigned long long r = bennett_measure(&p);
  r |= bennett_measure(&d) << 8;
  return r | bennett_measure(&c) << 16;
}
