/* Structure-pointer and error-order checks for vectored I/O syscalls (readv and writev) and for the data
 * buffers of read, write, getrandom and getcwd.
 *
 * argv[1] selects a case (or "agree" for the batch of non-faulting agreeing cases).
 * Optional argv[2] supplies the end-of-memory address on the device for partial-object cases;
 * natively, partial_edge maps a read/write page followed by a PROT_NONE guard page. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/uio.h>
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
    int rw = open("efault_iov.tmp", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (rw < 0) return 90;
    if (write(rw, "0123456789abcdef", 16) != 16) return 91;
    if (lseek(rw, 0, SEEK_SET) != 0) return 92;
    int ro = open("efault_iov.tmp", O_RDONLY);
    int wo = open("efault_iov.tmp", O_WRONLY);
    int closed_fd = dup(rw);
    if (ro < 0 || wo < 0 || closed_fd < 0 || close(closed_fd) != 0) return 93;
    unlink("efault_iov.tmp");

    char buf[16] = "abcdefghijklmnop";
    struct iovec good = { buf, 4 };
    struct iovec badlen = { buf, SIZE_MAX };
    struct iovec base_null = { NULL, 4 };
    void *low = (void *)(uintptr_t)8;
    void *wrap8 = (void *)(uintptr_t)-8;
    long r;

    if (!strcmp(c, "agree")) {
        char *edge = partial_edge(argc, argv);
        struct iovec *piov = (struct iovec *)(edge - sizeof(struct iovec));
        piov[0].iov_base = buf;
        piov[0].iov_len = SIZE_MAX;

        errno = 0; r = syscall(SYS_writev, rw, &good, 1);
        report("writev-valid", r, errno);
        errno = 0; r = syscall(SYS_writev, rw, &badlen, 1);
        report("writev-badlen", r, errno);
        errno = 0; r = syscall(SYS_writev, rw, NULL, 1025);
        report("writev-badcnt", r, errno);
        errno = 0; r = syscall(SYS_writev, closed_fd, NULL, 1);
        report("writev-badfd-null", r, errno);
        errno = 0; r = syscall(SYS_writev, rw, NULL, 1);
        report("writev-null", r, errno);
        errno = 0; r = syscall(SYS_writev, rw, piov, 2);
        report("writev-partial16-badlen0", r, errno);
        errno = 0; r = syscall(SYS_writev, rw, &base_null, 1);
        report("writev-base-null", r, errno);

        lseek(rw, 0, SEEK_SET);
        errno = 0; r = syscall(SYS_readv, rw, &good, 1);
        report("readv-valid", r, errno);
        errno = 0; r = syscall(SYS_readv, rw, &badlen, 1);
        report("readv-badlen", r, errno);
        errno = 0; r = syscall(SYS_readv, rw, NULL, 1025);
        report("readv-badcnt", r, errno);
        errno = 0; r = syscall(SYS_readv, closed_fd, NULL, 1);
        report("readv-badfd-null", r, errno);
        errno = 0; r = syscall(SYS_readv, rw, NULL, 1);
        report("readv-null", r, errno);
        errno = 0; r = syscall(SYS_readv, rw, piov, 2);
        report("readv-partial16-badlen0", r, errno);
        errno = 0; r = syscall(SYS_readv, rw, &base_null, 1);
        report("readv-base-null", r, errno);
        return 0;
    }

    if (!strcmp(c, "writev-low8")) {
        errno = 0; r = syscall(SYS_writev, rw, low, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-wrap8")) {
        errno = 0; r = syscall(SYS_writev, rw, wrap8, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(void **)(edge - 8) = NULL;
        errno = 0; r = syscall(SYS_writev, rw, edge - 8, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-partial16-valid0")) {
        char *edge = partial_edge(argc, argv);
        struct iovec *v = (struct iovec *)(edge - sizeof(struct iovec));
        v[0].iov_base = buf;
        v[0].iov_len = 4;
        errno = 0; r = syscall(SYS_writev, rw, v, 2);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-ro-null")) {
        errno = 0; r = syscall(SYS_writev, ro, NULL, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-ro-badlen")) {
        errno = 0; r = syscall(SYS_writev, ro, &badlen, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-ro-badcnt")) {
        errno = 0; r = syscall(SYS_writev, ro, NULL, 1025);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-ro-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_writev, ro, edge - 8, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-base-low8")) {
        struct iovec v = { low, 4 };
        errno = 0; r = syscall(SYS_writev, rw, &v, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "writev-base-partial4")) {
        char *edge = partial_edge(argc, argv);
        memcpy(edge - 4, "ABCD", 4);
        struct iovec v = { edge - 4, 8 };
        errno = 0; r = syscall(SYS_writev, rw, &v, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-low8")) {
        errno = 0; r = syscall(SYS_readv, rw, low, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-wrap8")) {
        errno = 0; r = syscall(SYS_readv, rw, wrap8, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-partial8")) {
        char *edge = partial_edge(argc, argv);
        *(void **)(edge - 8) = NULL;
        errno = 0; r = syscall(SYS_readv, rw, edge - 8, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-partial16-valid0")) {
        char *edge = partial_edge(argc, argv);
        struct iovec *v = (struct iovec *)(edge - sizeof(struct iovec));
        v[0].iov_base = buf;
        v[0].iov_len = 4;
        errno = 0; r = syscall(SYS_readv, rw, v, 2);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-wo-null")) {
        errno = 0; r = syscall(SYS_readv, wo, NULL, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-wo-badlen")) {
        errno = 0; r = syscall(SYS_readv, wo, &badlen, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-wo-badcnt")) {
        errno = 0; r = syscall(SYS_readv, wo, NULL, 1025);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-wo-partial8")) {
        char *edge = partial_edge(argc, argv);
        errno = 0; r = syscall(SYS_readv, wo, edge - 8, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-base-low8")) {
        struct iovec v = { low, 4 };
        errno = 0; r = syscall(SYS_readv, rw, &v, 1);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "readv-base-partial4")) {
        char *edge = partial_edge(argc, argv);
        struct iovec v = { edge - 4, 8 };
        errno = 0; r = syscall(SYS_readv, rw, &v, 1);
        report(c, r, errno); return 0;
    }

    if (!strcmp(c, "readv-alias")) {
        /* The first element reads over the second: Linux reads from its copy of the array. */
        if (lseek(rw, 16, SEEK_SET) != 16 || write(rw, "ghijklmnopqrstuv", 16) != 16 || lseek(rw, 0, SEEK_SET) != 0)
            return 94;
        struct iovec v[2];
        v[0].iov_base = &v[1];
        v[0].iov_len = sizeof v[1];
        v[1].iov_base = buf;
        v[1].iov_len = 4;
        errno = 0; r = syscall(SYS_readv, rw, v, 2);
        printf("%s: ret=%ld errno=%d buf=%.4s\n", c, r, errno, buf);
        return 0;
    }
    if (!strcmp(c, "read-low8")) {
        errno = 0; r = syscall(SYS_read, rw, low, 4);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "write-low8")) {
        errno = 0; r = syscall(SYS_write, rw, low, 4);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "getrandom-low8")) {
        errno = 0; r = syscall(SYS_getrandom, low, 4, 0);
        report(c, r, errno); return 0;
    }
    if (!strcmp(c, "getcwd-low8")) {
        errno = 0; r = syscall(SYS_getcwd, low, 4096);
        report(c, r, errno); return 0;
    }

    return 3;
}
