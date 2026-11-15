/* Harness self-test: scanf's %f with no floating-point operation in the program itself. With the no-FP scanf
 * patch the run must end in the named fault unreachable, never print something else. */
#include <stdio.h>
int main(void) {
  float f;
  printf("%d\n", sscanf("1.5", "%f", &f));
  return 0;
}
