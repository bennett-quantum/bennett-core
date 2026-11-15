#include <stdio.h>
#include <stdlib.h>
static void bye(void) { puts("atexit"); }
__attribute__((destructor)) static void fini(void) { puts("dtor"); }
__attribute__((noinline)) static void deep(void) { printf("buffered "); exit(37); }
int main(void) { atexit(bye); deep(); return 99; }
