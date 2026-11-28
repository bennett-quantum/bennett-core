/* Scalar system-call arguments with their upper 32 bits set. Linux takes each argument as the type its system
 * call declares (an int or unsigned int keeps its low 32 bits, a mode its low 16), so these calls behave as with
 * the low bits alone: descriptors, vector counts, clock ids, signal numbers, `how`, flags, modes and commands.
 *
 * argv[1] selects a case ("agree": every call). */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

/* Upper bits that an int or unsigned int argument drops. */
#define HIGH 0x5a5a5a5a00000000l

static void report(const char *label, long ret, int err)
{
    printf("%s: ret=%ld errno=%d\n", label, ret, err);
}

static volatile int delivered;
static void count(int sig)
{
    (void)sig;
    ++delivered;
}

int main(int argc, char **argv)
{
    if (argc < 2 || strcmp(argv[1], "agree")) return 2;
    umask(022); /* the device's, whatever the host's */
    int fd = open("wide_args.file", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (fd < 0) return 90;
    long r;
    char buf[8] = "abcdefgh";
    struct iovec v = { buf, 4 };
    struct stat st;

    /* Descriptors. */
    errno = 0; r = syscall(SYS_write, HIGH | fd, buf, 8); report("write", r, errno);
    errno = 0; r = syscall(SYS_lseek, HIGH | fd, 0l, HIGH | SEEK_SET); report("lseek", r, errno);
    errno = 0; r = syscall(SYS_read, HIGH | fd, buf, 2); report("read", r, errno);
    errno = 0; r = syscall(SYS_pread64, HIGH | fd, buf, 2, 4l); report("pread64", r, errno);
    errno = 0; r = syscall(SYS_pwrite64, HIGH | fd, buf, 2, 8l); report("pwrite64", r, errno);
    errno = 0; r = syscall(SYS_readv, HIGH | fd, &v, HIGH | 1); report("readv", r, errno);
    errno = 0; r = syscall(SYS_writev, HIGH | fd, &v, HIGH | 1); report("writev", r, errno);
    errno = 0; r = syscall(SYS_readv, fd, &v, HIGH); report("readv-count-high-only", r, errno);
    errno = 0; r = syscall(SYS_fstat, HIGH | fd, &st); report("fstat", r, errno);
    printf("fstat size=%ld\n", (long)st.st_size);
    errno = 0; r = syscall(SYS_fsync, HIGH | fd); report("fsync", r, errno);
    errno = 0; r = syscall(SYS_fdatasync, HIGH | fd); report("fdatasync", r, errno);
    errno = 0; r = syscall(SYS_ftruncate, HIGH | fd, 6l); report("ftruncate", r, errno);
    errno = 0; r = syscall(SYS_ioctl, HIGH | fd, HIGH | TIOCGWINSZ, buf); report("ioctl", r, errno);
    errno = 0; r = syscall(SYS_dup, HIGH | fd); report("dup", r, errno);
    errno = 0; r = syscall(SYS_dup2, HIGH | fd, HIGH | 10); report("dup2", r, errno);
    errno = 0; r = syscall(SYS_dup2, HIGH | fd, HIGH | fd); report("dup2-same", r, errno);
    errno = 0; r = syscall(SYS_dup3, HIGH | fd, HIGH | 11, HIGH | O_CLOEXEC); report("dup3", r, errno);
    errno = 0; r = syscall(SYS_dup3, HIGH | fd, HIGH | fd, 0l); report("dup3-same", r, errno);
    errno = 0; r = syscall(SYS_fcntl, HIGH | 11, HIGH | F_GETFD); report("fcntl-getfd", r, errno);
    errno = 0; r = syscall(SYS_fcntl, HIGH | 10, HIGH | F_SETFD, HIGH | FD_CLOEXEC); report("fcntl-setfd", r, errno);
    errno = 0; r = syscall(SYS_fcntl, 10, F_GETFD); report("fcntl-getfd-after", r, errno);
    errno = 0; r = syscall(SYS_fcntl, HIGH | fd, HIGH | F_SETFL, HIGH | O_APPEND); report("fcntl-setfl", r, errno);
    errno = 0; r = syscall(SYS_fcntl, fd, F_GETFL); printf("fcntl-getfl append=%d\n", r >= 0 && (r & O_APPEND));
    errno = 0; r = syscall(SYS_fcntl, HIGH | fd, HIGH | F_DUPFD, HIGH | 20); report("fcntl-dupfd", r, errno);
    errno = 0; r = syscall(SYS_fcntl, fd, F_DUPFD_CLOEXEC, HIGH | 30); report("fcntl-dupfd-cloexec", r, errno);
    errno = 0; r = syscall(SYS_close, HIGH | 20); report("close", r, errno);
    errno = 0; r = syscall(SYS_close, HIGH | 20); report("close-again", r, errno);
    int directory = open(".", O_RDONLY | O_DIRECTORY);
    errno = 0; r = syscall(SYS_fchdir, HIGH | directory); report("fchdir", r, errno);

    /* Directory descriptors, flags and modes of the path calls. */
    errno = 0; r = syscall(SYS_openat, HIGH | (unsigned)AT_FDCWD, "wide_args.file", HIGH | O_RDONLY);
    report("openat", r >= 0, errno);
    errno = 0; r = syscall(SYS_open, "wide_args.new", HIGH | O_CREAT | O_WRONLY, HIGH | 0640);
    report("open-creat", r >= 0, errno);
    errno = 0; r = syscall(SYS_stat, "wide_args.new", &st); printf("open-creat mode=%o\n", st.st_mode & 07777);
    errno = 0; r = syscall(SYS_chmod, "wide_args.new", HIGH | 0600); report("chmod", r, errno);
    errno = 0; r = syscall(SYS_stat, "wide_args.new", &st); printf("chmod mode=%o\n", st.st_mode & 07777);
    errno = 0; r = syscall(SYS_creat, "wide_args.creat", HIGH | 0604); report("creat", r >= 0, errno);
    errno = 0; r = syscall(SYS_stat, "wide_args.creat", &st); printf("creat mode=%o\n", st.st_mode & 07777);
    errno = 0; r = syscall(SYS_umask, HIGH | 077); report("umask", r, errno);
    errno = 0; r = syscall(SYS_umask, 022l); report("umask-back", r, errno);
    errno = 0; r = syscall(SYS_mkdir, "wide_args.dir", HIGH | 0750); report("mkdir", r, errno);
    errno = 0; r = syscall(SYS_mkdirat, HIGH | (unsigned)AT_FDCWD, "wide_args.dir2", HIGH | 0705);
    report("mkdirat", r, errno);
    errno = 0; r = syscall(SYS_stat, "wide_args.dir2", &st); printf("mkdirat mode=%o\n", st.st_mode & 07777);
    errno = 0; r = syscall(SYS_access, "wide_args.file", HIGH | R_OK); report("access", r, errno);
    errno = 0; r = syscall(SYS_faccessat, HIGH | (unsigned)AT_FDCWD, "wide_args.file", HIGH | F_OK);
    report("faccessat", r, errno);
    errno = 0; r = syscall(SYS_newfstatat, HIGH | (unsigned)AT_FDCWD, "wide_args.file", &st, HIGH);
    report("newfstatat", r, errno);
    errno = 0; r = syscall(SYS_readlink, "wide_args.file", buf, HIGH | 8); report("readlink", r, errno);
    errno = 0; r = syscall(SYS_readlinkat, HIGH | (unsigned)AT_FDCWD, "wide_args.file", buf, HIGH | 8);
    report("readlinkat", r, errno);
    errno = 0; r = syscall(SYS_unlinkat, HIGH | (unsigned)AT_FDCWD, "wide_args.dir", HIGH | AT_REMOVEDIR);
    report("unlinkat-dir", r, errno);
    errno = 0; r = syscall(SYS_unlinkat, HIGH | (unsigned)AT_FDCWD, "wide_args.new", HIGH); report("unlinkat", r, errno);
    rmdir("wide_args.dir2");
    unlink("wide_args.creat");
    unlink("wide_args.file");

    /* Clocks. */
    struct timespec ts = { 0, 0 };
    errno = 0; r = syscall(SYS_clock_gettime, HIGH | CLOCK_MONOTONIC, &ts); report("clock_gettime", r, errno);
    errno = 0; r = syscall(SYS_clock_gettime, HIGH | CLOCK_PROCESS_CPUTIME_ID, &ts);
    report("clock_gettime-cpu", r, errno);
    errno = 0; r = syscall(SYS_clock_getres, HIGH | CLOCK_REALTIME, &ts); report("clock_getres", r, errno);
    errno = 0; r = syscall(SYS_clock_getres, HIGH | 10, &ts); report("clock_getres-10", r, errno);
    ts.tv_sec = 0; ts.tv_nsec = 0;
    errno = 0; r = syscall(SYS_clock_nanosleep, HIGH | CLOCK_MONOTONIC, HIGH, &ts, NULL);
    report("clock_nanosleep", r, errno);
    errno = 0; r = syscall(SYS_getrandom, buf, 0l, HIGH | GRND_NONBLOCK); report("getrandom", r, errno);

    /* Signals. */
    struct sigaction action = { 0 };
    action.sa_handler = count;
    if (sigaction(SIGUSR1, &action, NULL)) return 91;
    unsigned long set = 1ul << (SIGUSR1 - 1), old = 0;
    errno = 0; r = syscall(SYS_rt_sigprocmask, HIGH | SIG_BLOCK, &set, NULL, 8l); report("sigprocmask", r, errno);
    errno = 0; r = syscall(SYS_kill, HIGH | getpid(), HIGH | SIGUSR1); report("kill", r, errno);
    printf("blocked delivered=%d\n", delivered);
    errno = 0; r = syscall(SYS_rt_sigprocmask, HIGH | SIG_UNBLOCK, &set, &old, 8l); report("sigprocmask-unblock", r, errno);
    printf("unblocked delivered=%d old=%lx\n", delivered, old);
    errno = 0; r = syscall(SYS_tkill, HIGH | gettid(), HIGH | SIGUSR1); report("tkill", r, errno);
    errno = 0; r = syscall(SYS_tgkill, HIGH | getpid(), HIGH | gettid(), HIGH | SIGUSR1); report("tgkill", r, errno);
    errno = 0; r = syscall(SYS_kill, HIGH | getpid(), HIGH); report("kill-0", r, errno);
    printf("delivered=%d\n", delivered);
    unsigned long stored[4] = { 0 };
    errno = 0; r = syscall(SYS_rt_sigaction, HIGH | SIGUSR1, NULL, stored, 8l); report("sigaction", r, errno);
    printf("sigaction handler-set=%d\n", stored[0] == (unsigned long)count);

    /* Memory advice. */
    char *page = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED) return 92;
    page[0] = 1;
    errno = 0; r = syscall(SYS_madvise, page, 4096l, HIGH | MADV_DONTNEED); report("madvise", r, errno);
    printf("madvise page[0]=%d\n", page[0]);
    fflush(stdout);
    syscall(SYS_exit_group, HIGH); /* the status is an int: 0 */
    return 93;
}
