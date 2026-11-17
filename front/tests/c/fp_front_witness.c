#include <math.h>
#include <stdint.h>
#include <string.h>

__attribute__((noinline)) unsigned bf_signbit(unsigned raw) {
  __bf16 x; memcpy(&x, &raw, 2); volatile __bf16 v = x;
  return __builtin_signbit(v) != 0;
}
__attribute__((noinline)) long unnormal_lround(uint64_t m, uint16_t e) {
  long double x = 0; memcpy(&x, &m, 8); memcpy((char *)&x + 8, &e, 2);
  volatile long double v = x;
  return lroundl(v);
}
__attribute__((noinline)) long long unnormal_llround(uint64_t m, uint16_t e) {
  long double x = 0; memcpy(&x, &m, 8); memcpy((char *)&x + 8, &e, 2);
  volatile long double v = x;
  return llroundl(v);
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  printf("%u %ld %lld\n", bf_signbit(0xbf80), unnormal_lround(0, 0x3fff), unnormal_llround(0, 0x3fff));
  return 0;
}
#endif
