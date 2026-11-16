/* Native values compared with every accepted coherent branch. */
#ifndef SHAPE
#define SHAPE 0
#endif
#if SHAPE == 0
#define N 8
#define A 8
#elif SHAPE == 1
#define N 16
#define A 8
#elif SHAPE == 2
#define N 17
#define A 32
#else
#define N 64
#define A 64
#endif
struct S { unsigned char b[N]; } __attribute__((aligned(A)));
#define NI __attribute__((noinline))
NI unsigned sum(struct S s) {
  unsigned h=0;
  for(unsigned i=0;i<N;i++) h=h*17+s.b[i];
  return h;
}
NI unsigned sibling(struct S s) { return sum(s); }
NI unsigned solve(unsigned k) {
  struct S s;
  for(unsigned i=0;i<N;i++) s.b[i]=k+3*i;
  return sibling(s)&255;
}
#ifdef NATIVE
#include <stdio.h>
int main(void) { for(unsigned k=0;k<8;k++) printf("%u\n",solve(k)); }
#else
#include "bennett.h"
unsigned ratio(void) {
  unsigned char q=0,y=0;
  bennett_h(bennett_bit(&q,0)); bennett_h(bennett_bit(&q,1)); bennett_h(bennett_bit(&q,2));
  y ^= solve(q);
  unsigned r=bennett_measure(&y);
  return r ^ bennett_measure(&q);
}
#endif
