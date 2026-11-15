#include <stdio.h>
#include <string.h>
int main(void) {
  char buf[64];
  int n = sprintf(buf, "x=%d y=%s", -17, "ab");
  return n == 10 && strcmp(buf, "x=-17 y=ab") == 0 ? 0 : 1;
}
