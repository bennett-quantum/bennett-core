#define _GNU_SOURCE
#include <sys/mman.h>
int main(void) { mremap(0,0,0,0); return 0; }
