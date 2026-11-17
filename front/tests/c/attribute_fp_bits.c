// Bit operations on floating-point values, which no subnormal mode changes: a function promising
// preserve-sign is accepted when it does only these. Native establishes the promised environment when FLUSH
// is set.
typedef unsigned long U;
__attribute__((noinline)) U result(unsigned shape, U input) {
  union { U u; double d; } a = {input}, r;
  switch (shape) {
  case 0: r.d = -a.d; return r.u;
  case 1: r.d = __builtin_fabs(a.d); return r.u;
  case 2: r.d = __builtin_copysign(1.5, a.d); return r.u;
  default: return 0;
  }
}
#ifdef NATIVE
#include <stdio.h>
#ifdef FLUSH
#include <xmmintrin.h>
#endif
int main(void) {
#ifdef FLUSH
  unsigned saved = _mm_getcsr();
  _mm_setcsr(saved | 0x8040); // DAZ and FTZ, signed zero on x86-64.
#endif
  const U inputs[] = {1,0x8000000000000001UL,0x10000000000000UL,0x8010000000000000UL,
                      0x800001UL,0x80000001UL,0x00800000UL,0x80800000UL};
  for (unsigned s=0;s<3;s++) for (unsigned i=0;i<8;i++) printf("%lx\n",result(s,inputs[i]));
#ifdef FLUSH
  _mm_setcsr(saved);
#endif
}
#endif
