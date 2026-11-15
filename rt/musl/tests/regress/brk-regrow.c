/* RTM008, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#define _GNU_SOURCE
#include <sys/syscall.h>
#include <unistd.h>
int main(void) {
 unsigned char *b=(void *)syscall(SYS_brk,0);
 if ((void *)syscall(SYS_brk,b+8192)!=b+8192) return 2;
 b[4096]=42;
 if ((void *)syscall(SYS_brk,b)!=b) return 3;
 if ((void *)syscall(SYS_brk,b+8192)!=b+8192) return 4;
 return b[4096]!=0;
}
