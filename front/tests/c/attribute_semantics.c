// Distinct values make extension, alias, byval-copy and hidden-result mistakes observable.
typedef unsigned long U;
#define NI __attribute__((noinline))
struct Big { U a, b, c; };
struct __attribute__((aligned(64))) Aligned { U a[8]; };
NI signed char narrow_s(signed char x) { return x / 3 - 7; }
NI unsigned char narrow_u(unsigned char x) { return x / 3 + 7; }
NI _Bool narrow_b(_Bool x) { return !x; }
NI struct Big hidden(struct Big x, U seed) {
  x.a ^= seed;
  x.b += 3 * seed;
  x.c = x.a + 7 * x.b;
  return x;
}
NI U aligned_copy(struct Aligned x) { x.a[0] += x.a[7]; return x.a[0] ^ x.a[3]; }
NI U pointer_facts(U *restrict out, const U *restrict in) __attribute__((nonnull));
NI U pointer_facts(U *restrict out, const U *restrict in) {
  U old = *in;
  *out = old + 19;
  return old ^ *out;
}
NI U *identity(U *p) { return p; }
#ifdef NO_BUILTIN
NI int abs(int x) { return x + 37; }
#endif
#ifdef NO_ABORT_BUILTIN
volatile U abort_value;
NI void abort(void) { abort_value = 137; }
#endif
NI U result(unsigned shape, U input) {
  switch (shape) {
  case 0: { signed char (*volatile f)(signed char) = narrow_s; return (U)(long)f((signed char)input); }
  case 1: { unsigned char (*volatile f)(unsigned char) = narrow_u; return f((unsigned char)input); }
  case 2: { _Bool (*volatile f)(_Bool) = narrow_b; return f((_Bool)input); }
  case 3: { struct Big a = {input, input ^ 0x13579UL, input + 17};
    struct Big b = hidden(a, input + 5); return a.a + 3*a.b + 7*a.c + 11*b.a + 13*b.b + 17*b.c; }
  case 4: { struct Aligned a = {{input, 2, 3, 17, 5, 6, 7, input ^ 0xabcUL}};
    U b = aligned_copy(a); return b + 3*a.a[0] + 7*a.a[7]; }
  case 5: { U a = 0, b = input; U r = pointer_facts(&a, &b); return r + 3*a + 7*b; }
  case 6: { U a = input; *identity(&a) += 29; return a; }
#ifdef NO_BUILTIN
  case 7: return (U)abs((int)(input & 127));
#endif
#ifdef NO_ABORT_BUILTIN
  case 7: abort_value = input; abort(); return abort_value;
#endif
  default: return 0;
  }
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  const U inputs[] = {0,1,127,128,255,256,0x123456789abcdefUL,~0UL};
#if defined(NO_BUILTIN) || defined(NO_ABORT_BUILTIN)
  const unsigned shapes = 8;
#else
  const unsigned shapes = 7;
#endif
  for (unsigned s=0;s<shapes;s++) for (unsigned i=0;i<8;i++) printf("%lx\n", result(s,inputs[i]));
}
#endif
