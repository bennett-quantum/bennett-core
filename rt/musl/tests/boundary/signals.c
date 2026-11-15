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
 unsigned long old=0, current=0, wanted=~0ul;
 CHECK(RAW(rt_sigprocmask,99,0,&old,8,0,0)==0);
 CHECK(RAW(rt_sigprocmask,99,&wanted,0,8,0,0)==-1 && errno==EINVAL);
 CHECK(RAW(rt_sigprocmask,SIG_SETMASK,&wanted,&old,1,0,0)==-1 && errno==EINVAL);
 CHECK(RAW(rt_sigprocmask,SIG_SETMASK,&wanted,&old,8,0,0)==0);
 CHECK(RAW(rt_sigprocmask,0,0,&current,8,0,0)==0);
 CHECK(current==(wanted & ~(1ul<<(SIGKILL-1)) & ~(1ul<<(SIGSTOP-1))));
 wanted=1ul<<(SIGUSR1-1);
 CHECK(RAW(rt_sigprocmask,SIG_UNBLOCK,&wanted,0,8,0,0)==0);
 CHECK(RAW(rt_sigprocmask,0,0,&current,8,0,0)==0 && !(current&wanted));
 CHECK(RAW(rt_sigprocmask,SIG_BLOCK,&wanted,0,8,0,0)==0);
 CHECK(RAW(rt_sigprocmask,0,0,&current,8,0,0)==0 && (current&wanted));
 wanted=0;
 CHECK(RAW(rt_sigprocmask,SIG_SETMASK,&wanted,0,8,0,0)==0);
 unsigned long act[4]={1,1,1,1};
 CHECK(RAW(rt_sigaction,SIGUSR1,0,act,8,0,0)==0 && !(act[0]|act[1]|act[2]|act[3]));
 CHECK(RAW(rt_sigaction,SIGUSR1,0,act,1,0,0)==-1 && errno==EINVAL);
 CHECK(getpid()==1 && RAW(gettid,0,0,0,0,0,0)==1);
 CHECK(RAW(set_tid_address,&current,0,0,0,0,0)==1);
 return 0;
}