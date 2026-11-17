// References give clang actual return/parameter nonnull, align and dereferenceable attributes.
using U = unsigned long;
#define NI __attribute__((noinline))
struct Box {
  U a, b;
  NI U read() const { return a + 7*b; }
};
NI Box &change(Box &x, U seed) { x.a ^= seed; x.b += 19; return x; }
NI const Box &choose(const Box &a, const Box &b, U which) { return which & 1 ? a : b; }
extern "C" NI U result(unsigned shape, U input) {
  Box a = {input, input ^ 0xabcUL}, b = {input + 17, input + 29};
  switch (shape) {
  case 0: return change(a, input + 3).read() + a.a;
  case 1: return choose(a, b, input).read();
  case 2: return a.read() + 3*b.read();
  default: return 0;
  }
}
#ifdef NATIVE
#include <stdio.h>
int main() {
  const U inputs[] = {0,1,127,128,255,256,0x123456789abcdefUL,~0UL};
  for (unsigned s=0;s<3;s++) for (unsigned i=0;i<8;i++) printf("%lx\n", result(s,inputs[i]));
}
#endif
