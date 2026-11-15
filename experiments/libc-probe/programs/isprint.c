#include <ctype.h>
int main(void) {
  int n = 0;
  for (int c = 0; c < 256; c++) n += isprint(c) != 0;
  return n == 95 ? 0 : 1;
}
