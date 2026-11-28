/* Valid structure-pointer calls in a small program, run on the device with an 8 KiB stack (--stack-bytes):
 * each call's copies live in its own frame, a vector's sized for its count, so no call pays for another's.
 * It reports with write alone (printf's floating-point formatting needs more than 8 KiB of stack on the
 * device) and uses the standard descriptors (opening a file, like the file model's other calls, needs more
 * too). argv[1] is "small-stack". */
#define _GNU_SOURCE
#include <signal.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

static void report(const char *label, long ret)
{
    char line[64], digits[24];
    size_t n = strlen(label), d = 0;
    unsigned long value = ret < 0 ? 0 - (unsigned long)ret : (unsigned long)ret;
    memcpy(line, label, n);
    line[n++] = ' ';
    if (ret < 0) line[n++] = '-';
    do digits[d++] = '0' + value % 10; while (value /= 10);
    while (d) line[n++] = digits[--d];
    line[n++] = '\n';
    if (write(1, line, n) != (long)n) _exit(91);
}

int main(int argc, char **argv)
{
    if (argc < 2 || strcmp(argv[1], "small-stack")) return 2;
    struct timespec ts;
    long tv[2];
    report("clock_gettime", syscall(SYS_clock_gettime, CLOCK_MONOTONIC, &ts));
    report("clock_getres", syscall(SYS_clock_getres, CLOCK_MONOTONIC, &ts));
    report("gettimeofday", syscall(SYS_gettimeofday, tv, NULL));
    char buf[5] = "abc\n";
    struct iovec v[2] = { { buf, 2 }, { buf + 2, 2 } };
    report("writev", syscall(SYS_writev, 1, v, 2));
    report("readv", syscall(SYS_readv, 0, v, 2)); /* empty stdin */
    unsigned long set = 0, old;
    report("rt_sigprocmask", syscall(SYS_rt_sigprocmask, SIG_BLOCK, &set, &old, 8));
    report("rt_sigpending", syscall(SYS_rt_sigpending, &old, 8));
    return 0;
}
