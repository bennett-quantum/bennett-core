/* RTM007, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#define _GNU_SOURCE
#include <sys/mman.h>
int main(void) { mremap(0,0,0,0); return 0; }
