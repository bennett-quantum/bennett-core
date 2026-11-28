/* Structure-pointer and error-order checks for stat syscalls:
 * stat, lstat, fstat, and newfstatat.
 *
 * argv[1] selects a case (or "agree" for the batch of non-faulting agreeing cases).
 * Optional argv[2] supplies the end-of-memory address on the device for partial-object cases;
 * natively, partial_edge maps a read/write page followed by a PROT_NONE guard page. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

static char *partial_edge(int argc, char **argv)
{
    if (argc >= 3)
        return (char *)(uintptr_t)strtoull(argv[2], NULL, 10);
    char *pages = mmap(NULL, 8192, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (pages == MAP_FAILED) _exit(99);
    if (mprotect(pages + 4096, 4096, PROT_NONE) != 0) _exit(98);
    return pages + 4096;
}

static void report(const char *label, long ret, int err)
{
    printf("%s: ret=%ld errno=%d\n", label, ret, err);
}

int main(int argc, char **argv)
{
    if (argc < 2) return 2;
    const char *c = argv[1];
    struct stat st;
    void *low = (void *)(uintptr_t)8;
    void *wrap8 = (void *)(uintptr_t)-8;
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) return 90;
    int closed_fd = dup(fd);
    if (closed_fd < 0 || close(closed_fd) != 0) return 91;
    long r;

    if (!strcmp(c, "agree")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_stat, "/dev/null", &st);
        report("stat-valid", r, errno);
        errno = 0; r = syscall(SYS_stat, "/no_such_efault_path", &st);
        report("stat-badpath-valid", r, errno);
        errno = 0; r = syscall(SYS_stat, "/no_such_efault_path", NULL);
        report("stat-badpath-null", r, errno);
        errno = 0; r = syscall(SYS_stat, "/no_such_efault_path", edge - 8);
        report("stat-badpath-partial8", r, errno);
        errno = 0; r = syscall(SYS_stat, "/dev/null", NULL);
        report("stat-null", r, errno);

        errno = 0; r = syscall(SYS_lstat, "/dev/null", &st);
        report("lstat-valid", r, errno);
        errno = 0; r = syscall(SYS_lstat, "/no_such_efault_path", &st);
        report("lstat-badpath-valid", r, errno);
        errno = 0; r = syscall(SYS_lstat, "/dev/null", NULL);
        report("lstat-null", r, errno);

        errno = 0; r = syscall(SYS_fstat, fd, &st);
        report("fstat-valid", r, errno);
        errno = 0; r = syscall(SYS_fstat, closed_fd, &st);
        report("fstat-badfd-valid", r, errno);
        errno = 0; r = syscall(SYS_fstat, closed_fd, NULL);
        report("fstat-badfd-null", r, errno);
        errno = 0; r = syscall(SYS_fstat, closed_fd, edge - 8);
        report("fstat-badfd-partial8", r, errno);
        errno = 0; r = syscall(SYS_fstat, fd, NULL);
        report("fstat-null", r, errno);

        errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, "/dev/null", &st, 0);
        report("newfstatat-valid", r, errno);
        errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, "/no_such_efault_path", &st, 0);
        report("newfstatat-badpath-valid", r, errno);
        errno = 0; r = syscall(SYS_newfstatat, closed_fd, "rel", &st, 0);
        report("newfstatat-baddirfd-valid", r, errno);
        errno = 0; r = syscall(SYS_newfstatat, closed_fd, "rel", edge - 8, 0);
        report("newfstatat-baddirfd-partial8", r, errno);
        errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, "/dev/null", NULL, 0);
        report("newfstatat-null", r, errno);
        return 0;
    }

    if (!strcmp(c, "stat-low8")) {
        errno = 0; r = syscall(SYS_stat, "/dev/null", low);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "stat-wrap8")) {
        errno = 0; r = syscall(SYS_stat, "/dev/null", wrap8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "stat-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_stat, "/dev/null", edge - 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "stat-partial136")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_stat, "/dev/null", edge - 136);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "lstat-low8")) {
        errno = 0; r = syscall(SYS_lstat, "/dev/null", low);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "lstat-wrap8")) {
        errno = 0; r = syscall(SYS_lstat, "/dev/null", wrap8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "lstat-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_lstat, "/dev/null", edge - 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "fstat-low8")) {
        errno = 0; r = syscall(SYS_fstat, fd, low);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "fstat-wrap8")) {
        errno = 0; r = syscall(SYS_fstat, fd, wrap8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "fstat-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_fstat, fd, edge - 8);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "newfstatat-low8")) {
        errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, "/dev/null", low, 0);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "newfstatat-wrap8")) {
        errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, "/dev/null", wrap8, 0);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "newfstatat-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, "/dev/null", edge - 8, 0);
        report(c, r, errno); return 0;
    }

    return 3;
}
