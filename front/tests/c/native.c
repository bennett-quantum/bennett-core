/* native.c: the corpus compiled natively. Usage: native FUNCTION < inputs; prints f(x) for each input. */
#include <stdio.h>
#include <string.h>

#include "corpus.c"

typedef int (*function)(unsigned char);

#define ENTRY(f) {#f, f},
static const struct {
  const char *name;
  function call;
} entries[] = {CORPUS(ENTRY)};

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  for (size_t i = 0; i < sizeof entries / sizeof entries[0]; i++) {
    if (strcmp(entries[i].name, argv[1]) == 0) {
      unsigned x;
      while (scanf("%u", &x) == 1) printf("%u\n", (unsigned)entries[i].call((unsigned char)x));
      return 0;
    }
  }
  return 2;
}
