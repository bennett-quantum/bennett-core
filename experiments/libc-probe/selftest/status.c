/* Harness self-test: same output on both, exit status 0 natively; the device result must agree. A second line
 * through stdio buffering checks the flush on return from main. */
#include <stdio.h>
int main(void) {
  fputs("one\n", stdout);
  printf("%s %d\n", "two", 2);
  return 0;
}
