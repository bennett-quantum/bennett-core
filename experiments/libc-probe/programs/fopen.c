#include <stdio.h>
int main(void) {
  FILE *f = fopen("/nonexistent-dir/file", "r");
  if (f) return 1;
  fputs("no file\n", stdout);
  return 0;
}
