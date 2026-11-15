/* RTMD001, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#include <stdlib.h>
int main(void) { void *p = malloc(1); free(p); return 0; }
