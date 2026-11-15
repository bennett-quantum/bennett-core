/* RTM004, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#define _GNU_SOURCE
#include <errno.h>
#include <unistd.h>
#include <sys/syscall.h>
int main(void) { long action[4]; return !(syscall(SYS_rt_sigaction, 0, 0, action, 8) == -1 && errno == EINVAL); }
