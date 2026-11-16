/* Independent by-value inputs: ABI size boundaries, over-alignment, local-copy
 * mutation and direct/indirect/sibling/recursive call paths. Only fields read;
 * padding is never part of the oracle. All arithmetic is unsigned. */
typedef unsigned long U;
#define NI __attribute__((noinline))
#define SHAPES(X) \
  X(0,1,1) X(1,2,1) X(2,3,1) X(3,7,1) X(4,8,8) X(5,9,1) \
  X(6,15,1) X(7,16,8) X(8,17,1) X(9,24,8) X(10,31,1) X(11,32,16) \
  X(12,33,1) X(13,64,8) X(14,127,1) X(15,17,32) X(16,33,64) \
  X(17,65,128) X(18,129,256) X(19,17,4096) \
  X(20,4,4) X(21,6,2) X(22,17,8192) X(23,17,16384) \
  X(24,17,512) X(25,17,1024) X(26,17,2048)
#define DEFINE(K,N,A) \
  struct S##K { unsigned char b[N]; } __attribute__((aligned(A))); \
  NI U sum##K(struct S##K s) { \
    U h = 0; for (unsigned i=0;i<N;i++) h = h*257+s.b[i]; return h; \
  } \
  NI U sibling##K(struct S##K s) { return sum##K(s); } \
  NI U recur##K(struct S##K s, unsigned depth) { \
    if (!depth) return sum##K(s); \
    s.b[depth % N] ^= depth; return recur##K(s,depth-1) + sum##K(s); \
  } \
  NI U change##K(struct S##K s, struct S##K *original) { \
    s.b[0] ^= 255; return sum##K(s) ^ sum##K(*original); \
  } \
  static U (*volatile fp##K)(struct S##K) = sum##K; \
  NI U case##K(U x, unsigned mode) { \
    struct S##K s; \
    for(unsigned i=0;i<N;i++) s.b[i] = (x >> (i%8)*8) + 19*i; \
    switch(mode) { \
    case 0: return sum##K(s); \
    case 1: return fp##K(s); \
    case 2: return sibling##K(s); \
    case 3: return recur##K(s,3); \
    default: { U v=change##K(s,&s); return v + sum##K(s); } \
    } \
  }
SHAPES(DEFINE)
U result(unsigned shape, U x, unsigned mode) {
  switch(shape) {
#define CASE(K,N,A) case K: return case##K(x,mode);
    SHAPES(CASE)
  default: return 0;
  }
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  const U inputs[] = {0,1,7,0x7fffffffffffffffUL,0x8000000000000000UL,~0UL,
                      0x0123456789abcdefUL,0xfedcba9876543210UL};
  for(unsigned s=0;s<27;s++) for(unsigned m=0;m<5;m++) for(unsigned i=0;i<8;i++)
    printf("%lx\n", result(s,inputs[i],m));
}
#endif
