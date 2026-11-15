#define _GNU_SOURCE
#include <sys/mman.h>
int main(void) { unsigned char *p = mmap(0,4096,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0); if (p == MAP_FAILED) return 2; p[1]=42; if (madvise(p,1,MADV_DONTNEED)) return 3; return p[1] != 0; }
