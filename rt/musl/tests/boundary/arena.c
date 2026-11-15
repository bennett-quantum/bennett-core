#define _GNU_SOURCE
#include <unistd.h>
#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/uio.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#define RAW(n,a,b,c,d,e,f) syscall(SYS_##n,(long)(a),(long)(b),(long)(c),(long)(d),(long)(e),(long)(f))
#define CHECK(e) do { if (!(e)) return __LINE__; } while (0)
int main(void) {
 uintptr_t b=RAW(brk,0,0,0,0,0,0);
 CHECK(!(b&4095));
 CHECK(RAW(brk,b-1,0,0,0,0,0)==b);
 CHECK(RAW(brk,UINTPTR_MAX,0,0,0,0,0)==b);
 CHECK(mmap(0,0,3,MAP_PRIVATE|MAP_ANONYMOUS,-1,0)==MAP_FAILED && errno==EINVAL);
 CHECK(mmap(0,SIZE_MAX,3,MAP_PRIVATE|MAP_ANONYMOUS,-1,0)==MAP_FAILED && errno==ENOMEM);
 unsigned char *p=mmap(0,1,3,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
 CHECK((uintptr_t)p==b+12288 && p[0]==0 && p[4095]==0);
 p[0]=12; p[4095]=34;
 CHECK(RAW(brk,b+12288,0,0,0,0,0)==b+12288);
 CHECK(RAW(brk,b+12289,0,0,0,0,0)==b+12288);
 CHECK(mmap(0,1,3,MAP_PRIVATE|MAP_ANONYMOUS,-1,0)==MAP_FAILED && errno==ENOMEM);
 CHECK(RAW(brk,b+4096,0,0,0,0,0)==b+4096);
 unsigned char *q=mmap(0,4097,3,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
 CHECK((uintptr_t)q==b+4096 && q[0]==0 && q[8191]==0);
 q[0]=56; q[8191]=78;
 CHECK(p[0]==12 && p[4095]==34);
 CHECK(mprotect(p,4096,3)==0);
 CHECK(RAW(mprotect,p+1,1,3,0,0,0)==-1 && errno==EINVAL);
 CHECK(madvise(p,4096,MADV_DONTNEED)==0 && p[0]==0 && p[4095]==0);
 CHECK(madvise(p+1,1,MADV_DONTNEED)==-1 && errno==EINVAL);
 CHECK(RAW(mremap,p,4096,8192,MREMAP_MAYMOVE,0,0)==-1 && errno==ENOMEM);
 CHECK(munmap(p+1,1)==-1 && errno==EINVAL);
 CHECK(munmap(p,0)==-1 && errno==EINVAL);
 CHECK(munmap(p,1)==0);
 CHECK(mmap(0,4096,3,MAP_PRIVATE|MAP_ANONYMOUS,-1,0)==MAP_FAILED && errno==ENOMEM);
 return 0;
}