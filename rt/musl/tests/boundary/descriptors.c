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
 char ch='x';
 CHECK(write(1,"",0)==0);
 CHECK(write(-1,"x",1)==-1 && errno==EBADF);
 CHECK(write(0,"x",1)==-1 && errno==EBADF);
 struct iovec v[3]={{"A",1},{0,0},{"BC",2}};
 CHECK(writev(1,v,3)==3);
 CHECK(writev(1,0,0)==0);
 CHECK(writev(1,v,-1)==-1 && errno==EINVAL);
 CHECK(writev(1,v,1025)==-1 && errno==EINVAL);
 CHECK(write(1,"",(size_t)LONG_MAX+1)==-1 && errno==EINVAL);
 CHECK(read(0,&ch,0)==0 && ch=='x');
 CHECK(read(0,&ch,1)==0 && ch=='x');
 CHECK(read(1,&ch,1)==-1 && errno==EBADF);
 CHECK(ioctl(1,TIOCGWINSZ,&ch)==-1 && errno==ENOTTY);
 CHECK(lseek(1,0,SEEK_CUR)==-1 && errno==ESPIPE);
 CHECK(close(1)==0);
 CHECK(write(1,"x",1)==-1 && errno==EBADF);
 CHECK(close(1)==-1 && errno==EBADF);
 CHECK(close(0)==0);
 CHECK(read(0,&ch,1)==-1 && errno==EBADF);
 CHECK(close(2)==0);
 CHECK(write(2,"x",1)==-1 && errno==EBADF);
 return 0;
}