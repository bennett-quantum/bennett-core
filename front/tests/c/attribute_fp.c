// Native establishes the environment promised by denormal-fp-math when FLUSH is set.
#ifdef STRICT
#pragma STDC FENV_ACCESS ON
#endif
typedef unsigned long U;
__attribute__((noinline)) double half(double x) { return x * 0.5; }
__attribute__((noinline)) U result(unsigned shape, U input) {
#ifdef F32_ONLY
  shape += 2;
#endif
  union { U u; double d; } a = {input}, r;
  union { unsigned u; float f; } b = {(unsigned)input}, s;
  switch (shape) {
  case 0: r.d = a.d + a.d; return r.u;
  case 1: r.d = a.d * 0.5; return r.u;
  case 2: s.f = b.f + b.f; return s.u;
  case 3: s.f = b.f * 0.5f; return s.u;
  case 4: r.d = (a.d + 1.0) - 1.0; return r.u;
  case 5: r.d = half(a.d); return r.u;
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
#ifdef F32_ONLY
  const unsigned shapes = 2;
#else
  const unsigned shapes = 6;
#endif
  for (unsigned s=0;s<shapes;s++) for (unsigned i=0;i<8;i++) printf("%lx\n",result(s,inputs[i]));
#ifdef FLUSH
  _mm_setcsr(saved);
#endif
}
#endif
