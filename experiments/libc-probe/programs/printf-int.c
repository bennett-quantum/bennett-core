#include <stdio.h>
int main(void) {
  printf("%d|%i|%u|%x|%X|%c|%s|%%\n", -42, 7, 4294967295u, 0xabcdef, 0xabcdef, 'Z', "str");
  printf("[%6d][%-6d][%06d][%.3d][%+d]\n", 42, 42, -42, 5, 9);
  printf("%ld|%lld|%llu|%lx|%p\n", -9223372036854775807L - 1, 123456789012345LL, 18446744073709551615ULL,
         0xdeadbeefUL, (void *)0x1234);
  return 3;
}
