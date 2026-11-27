#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>
#include <sys/time.h>
#include <sys/random.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <fcntl.h>
#include <unistd.h>
/* For the native comparison only: an independent arithmetic oracle for the input clock and tape.
 * Production device code uses musl's syscall routes; this does not import runtime implementation. */
#ifdef NATIVE_INPUTS
static uint64_t tick, slept;
static size_t pos;
static int model_clock(clockid_t id,struct timespec *ts) {
    if(id==99) { errno=EINVAL; return -1; }
    uint64_t n=START+tick++ +(id==CLOCK_PROCESS_CPUTIME_ID?0:slept); ts->tv_sec=n/1000000000; ts->tv_nsec=n%1000000000; return 0;
}
/* A sleep adds its duration to the clocks that count while the process sleeps, modulo 2^64; an absolute one
 * adds what is left to the target. */
static int model_clock_nanosleep(clockid_t id,int flags,const struct timespec *req,struct timespec *rem) {
    (void)id; (void)rem;
    uint64_t wanted=(uint64_t)req->tv_sec*1000000000+req->tv_nsec, now=START+tick+slept;
    if(flags&TIMER_ABSTIME) wanted=wanted<=now?0:wanted-now;
    slept+=wanted; return 0;
}
static int model_nanosleep(const struct timespec *req,struct timespec *rem) {
    return model_clock_nanosleep(CLOCK_REALTIME,0,req,rem);
}
static int model_timeval(struct timeval *tv,void *zone) {
    (void)zone; struct timespec ts; model_clock(0,&ts); tv->tv_sec=ts.tv_sec; tv->tv_usec=ts.tv_nsec/1000; return 0;
}
static time_t model_time(time_t *t) {
    struct timespec ts; model_clock(0,&ts); if(t) *t=ts.tv_sec; return ts.tv_sec;
}
static ssize_t model_random(void *p,size_t n,unsigned flags) {
    if(flags&8) { errno=EINVAL; return -1; }
    for(size_t i=0;i<n;++i) ((unsigned char *)p)[i]=(unsigned char)(17*(pos+i)+3);
    pos+=n; return n;
}
#define clock_gettime model_clock
#define gettimeofday model_timeval
#define time model_time
#define getrandom model_random
#define nanosleep model_nanosleep
#define clock_nanosleep model_clock_nanosleep
#endif
#ifndef START
#define START UINT64_C(1234999999998)
#endif
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    struct timespec ts; struct timeval tv; time_t t;
    CHECK(clock_gettime(CLOCK_REALTIME,&ts)==0);
    printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    CHECK(clock_gettime(CLOCK_MONOTONIC,&ts)==0);
    printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    CHECK(gettimeofday(&tv,0)==0); printf("%lld %ld\n",(long long)tv.tv_sec,tv.tv_usec);
    printf("%lld\n",(long long)time(&t));
    errno=0; CHECK(clock_gettime(99,&ts)==-1 && errno==EINVAL);
    CHECK(clock_gettime(CLOCK_REALTIME,&ts)==0);
    printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    unsigned char b[16];
    CHECK(getrandom(b,7,0)==7 && getrandom(b,0,0)==0);
    for(int i=0;i<7;++i) printf("%02x",b[i]); puts("");
    errno=0; CHECK(getrandom(b,1,8)==-1 && errno==EINVAL);
    CHECK(getrandom(b,9,0)==9);
    for(int i=0;i<9;++i) printf("%02x",b[i]); puts("");
    /* Sleeping advances the clocks by exactly the duration, wrapping modulo 2^64; a CPU-time clock not at all. */
    struct timespec d={1,5}, rem={77,77};
    CHECK(nanosleep(&d,&rem)==0 && rem.tv_sec==77 && rem.tv_nsec==77);
    CHECK(clock_gettime(CLOCK_MONOTONIC,&ts)==0); printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    CHECK(clock_gettime(CLOCK_PROCESS_CPUTIME_ID,&ts)==0); printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    CHECK(clock_gettime(CLOCK_MONOTONIC,&ts)==0);
    ts.tv_sec+=2; ts.tv_nsec=7;
    CHECK(clock_nanosleep(CLOCK_MONOTONIC,TIMER_ABSTIME,&ts,0)==0);
    CHECK(clock_gettime(CLOCK_REALTIME,&ts)==0); printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    struct timespec past={0,3};
    CHECK(clock_nanosleep(CLOCK_REALTIME,TIMER_ABSTIME,&past,0)==0);
    struct timespec most={18446744073,0}; /* just under 2^64 ns: the clock wraps */
    CHECK(clock_nanosleep(CLOCK_BOOTTIME,0,&most,0)==0);
    CHECK(gettimeofday(&tv,0)==0); printf("%lld %ld\n",(long long)tv.tv_sec,tv.tv_usec);
    printf("%lld\n",(long long)time(0));
    CHECK(clock_gettime(CLOCK_PROCESS_CPUTIME_ID,&ts)==0); printf("%lld %ld\n",(long long)ts.tv_sec,ts.tv_nsec);
    return 0;
}
