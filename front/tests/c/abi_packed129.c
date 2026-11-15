/* ABI-D1: a valid 17-byte bitfield result; LLVM merges its copy to i136, above
 * the device width. This is a named refusal, not a wrong numeric answer. Keep
 * the native oracle for removing the debt. */
typedef unsigned long long u64;
struct __attribute__((packed)) value {
  u64 a : 64, b : 64;
  unsigned c : 1;
};
__attribute__((noinline)) struct value make(u64 x) {
  return (struct value){x, ~x, (unsigned)(x & 1)};
}
u64 result(u64 x) {
  struct value p = make(x);
  return p.a ^ (p.b * 3) ^ p.c;
}
#ifdef NATIVE
int printf(const char *, ...);
int main(void) {
  const u64 inputs[] = {0,
                        1,
                        7,
                        0x7fffffffffffffffULL,
                        0x8000000000000000ULL,
                        0xffffffffffffffffULL,
                        0x0123456789abcdefULL,
                        0xfedcba9876543210ULL};
  for (unsigned i = 0; i < sizeof inputs / sizeof inputs[0]; ++i)
    printf("%016llx\n", result(inputs[i]));
  return 0;
}
#endif
