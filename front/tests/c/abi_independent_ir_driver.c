/* Native oracle for abi_independent.ll; return types at this boundary match
 * clang's C ABI. */
typedef unsigned long long u64;
typedef unsigned __int128 u128;
u128 result(unsigned, u64, unsigned);
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
  for (unsigned i = 0; i < 12; ++i)
    for (unsigned mode = 0; mode < 2; ++mode)
      for (unsigned j = 0; j < sizeof inputs / sizeof inputs[0]; ++j) {
        u128 r = result(i, inputs[j], mode);
        printf("%016llx%016llx\n", (u64)(r >> 64), (u64)r);
      }
  return 0;
}
