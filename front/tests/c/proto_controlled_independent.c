/* Native basis values of accepted prototype mismatches, including an indirect table. */
typedef unsigned long U;
#define NI __attribute__((noinline))
#ifndef SHAPE
#define SHAPE 0
#endif
#if SHAPE == 0
NI unsigned target(unsigned x) { return x*13+7; }
NI unsigned solve(unsigned x) { return ((unsigned (*)(U,U))target)((U)x,(U)99); }
#elif SHAPE == 1
struct P { U a,b; };
NI struct P target(U x) { struct P s={x*17+1,x+19};return s; }
NI unsigned solve(unsigned x) { return (unsigned)((U (*)(U))target)((U)x); }
#elif SHAPE == 2
NI U target(unsigned n) { return n ? n+((U (*)(U,U))target)((U)(n-1),(U)99) : 0; }
NI unsigned solve(unsigned x) { return (unsigned)((U (*)(U,U))target)((U)x,(U)99); }
#else
NI unsigned a(unsigned x) { return x*7+3; }
NI unsigned b(unsigned short x) { return x*13+9; }
static unsigned (*volatile table[2])(U)={(unsigned (*)(U))a,(unsigned (*)(U))b};
NI unsigned solve(unsigned x) { return table[x&1]((U)x); }
#endif
#ifdef NATIVE
#include <stdio.h>
int main(void) { for(unsigned i=0;i<8;i++) printf("%u\n",solve(i)); }
#else
#include "bennett.h"
unsigned ratio(void) {
  unsigned char q=0,y=0;
  bennett_ry(bennett_bit(&q,0),0.6);
  bennett_h(bennett_bit(&q,1));bennett_h(bennett_bit(&q,2));
  y ^= solve(q);
  unsigned r=bennett_measure(&y);
  return r ^ bennett_measure(&q);
}
#endif
