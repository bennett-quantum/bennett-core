/* The CPU-time clocks of the caller, by the ids musl builds for clock_getcpuclockid and
 * pthread_getcpuclockid. Before the fix they failed with ESRCH and EINVAL. */
#include <errno.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    clockid_t id; struct timespec t;
    CHECK(clock_getcpuclockid(0,&id)==0 && clock_gettime(id,&t)==0 && clock_getres(id,&t)==0);
    CHECK(clock_getcpuclockid(getpid(),&id)==0 && clock_gettime(id,&t)==0);
    CHECK(pthread_getcpuclockid(pthread_self(),&id)==0 && clock_gettime(id,&t)==0);
    CHECK(clock_gettime(-3,&t)==0); /* the calling thread's virtual time: ~(-3>>3) is 0, kind 1, per thread */
    errno=0; CHECK(clock_gettime(-1,&t)==-1 && errno==EINVAL); /* kind 3 */
    errno=0; CHECK(clock_gettime(-5,&t)==-1 && errno==EINVAL); /* a clock behind fd 0, not a clock device */
    CHECK(clock_getcpuclockid(100000000,&id)==ESRCH);
    return 0;
}
