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
    if (!strcmp(c, "writev-wrap-second") || !strcmp(c, "readv-wrap-second")) {
        /* Every element's range is checked before any byte moves: nothing is transferred, the offset stays. */
        int writing = c[0] == 'w';
        struct iovec v[2] = { { buf, 1 }, { (void *)(uintptr_t)-2, 4 } };
        errno = 0; r = syscall(writing ? SYS_writev : SYS_readv, rw, v, 2);
        int e = errno;
        printf("%s: ret=%ld errno=%d offset=%ld buf=%c\n", c, r, e, (long)lseek(rw, 0, SEEK_CUR), buf[0]);
        return 0;
    }
    if (!strcmp(c, "low-untouched")) {
        /* A buffer below 16 faults only where bytes are transferred: none at the end of a file, for a directory,
         * for /dev/null, for empty stdin; /dev/zero transfers. */
        int null = open("/dev/null", O_RDWR), zero = open("/dev/zero", O_RDONLY), dir = open(".", O_RDONLY | O_DIRECTORY);
        if (null < 0 || zero < 0 || dir < 0 || lseek(rw, 0, SEEK_END) != 16) return 95;
        struct iovec v = { low, 1 };
        errno = 0; r = syscall(SYS_read, rw, low, 1); report("eof-read", r, errno);
        errno = 0; r = syscall(SYS_readv, rw, &v, 1); report("eof-readv", r, errno);
        errno = 0; r = syscall(SYS_write, null, low, 1); report("null-write", r, errno);
        errno = 0; r = syscall(SYS_read, null, low, 1); report("null-read", r, errno);
        errno = 0; r = syscall(SYS_writev, null, &v, 1); report("null-writev", r, errno);
        errno = 0; r = syscall(SYS_readv, null, &v, 1); report("null-readv", r, errno);
        errno = 0; r = syscall(SYS_read, dir, low, 1); report("dir-read", r, errno);
        errno = 0; r = syscall(SYS_readv, dir, &v, 1); report("dir-readv", r, errno);
        errno = 0; r = syscall(SYS_read, 0, low, 1); report("stdin-read", r, errno);
        errno = 0; r = syscall(SYS_read, zero, low, 1); report("zero-read", r, errno);
        errno = 0; r = syscall(SYS_write, rw, low, 1); report("file-write", r, errno);
        struct iovec two[2] = { { buf, 1 }, { low, 1 } };
        errno = 0; r = syscall(SYS_writev, null, two, 2); report("null-writev-second", r, errno);
        errno = 0; r = syscall(SYS_writev, rw, two, 2); report("file-writev-second", r, errno);
        return 0;
    }
    if (!strcmp(c, "ranges-and-cap")) {
        /* Linux's range check (ends at most 2^47 - 4096, an empty range above it refused too) and its cap of
         * 0x7ffff000 bytes: read and write check before capping, getrandom and a one-element vector cap first,
         * several elements are each checked at full length after all are copied, then capped. */
        int null = open("/dev/null", O_RDWR), dir = open(".", O_RDONLY | O_DIRECTORY);
        if (null < 0 || dir < 0) return 95;
        char *top = (char *)(uintptr_t)0x7ffffffff000ull;
        static char area[16]; /* static: natively far below the top, where a stack buffer may not be */
        errno = 0; r = syscall(SYS_write, null, area, 1ul << 31); report("write-cap", r, errno);
        errno = 0; r = syscall(SYS_write, null, area, (size_t)LONG_MAX); report("write-longmax", r, errno);
        errno = 0; r = syscall(SYS_write, null, area, (size_t)LONG_MAX + 1); report("write-above-longmax", r, errno);
        errno = 0; r = syscall(SYS_read, null, area, 1ul << 47); report("read-2^47", r, errno);
        errno = 0; r = syscall(SYS_write, null, top - 8, 8); report("write-to-top", r, errno);
        errno = 0; r = syscall(SYS_write, null, top - 8, 9); report("write-past-top", r, errno);
        errno = 0; r = syscall(SYS_write, null, top, 0); report("write-top-empty", r, errno);
        errno = 0; r = syscall(SYS_write, null, wrap8, 0); report("write-high-empty", r, errno);
        errno = 0; r = syscall(SYS_getrandom, top - 4096, (size_t)LONG_MAX + 1, 0); report("getrandom-cap-top", r, errno);
        errno = 0; r = syscall(SYS_getrandom, wrap8, 0, 0); report("getrandom-high-empty", r, errno);
        struct iovec one = { area, 0x80000000ul };
        errno = 0; r = syscall(SYS_writev, null, &one, 1); report("writev-cap-one", r, errno);
        one.iov_len = LONG_MAX;
        errno = 0; r = syscall(SYS_writev, null, &one, 1); report("writev-longmax-one", r, errno);
        one.iov_base = top - 4096; one.iov_len = 0x80000000ul;
        errno = 0; r = syscall(SYS_writev, null, &one, 1); report("writev-capped-past-top", r, errno);
        one.iov_base = wrap8; one.iov_len = 0;
        errno = 0; r = syscall(SYS_writev, null, &one, 1); report("writev-high-empty", r, errno);
        struct iovec two[2] = { { area, 0x7ffff000ul }, { area, 1 } };
        errno = 0; r = syscall(SYS_writev, null, two, 2); report("writev-cap-two", r, errno);
        two[0].iov_len = LONG_MAX;
        errno = 0; r = syscall(SYS_writev, null, two, 2); report("writev-sum", r, errno);
        two[0].iov_len = 1; two[1].iov_base = wrap8; two[1].iov_len = 0;
        errno = 0; r = syscall(SYS_writev, null, two, 2); report("writev-high-empty-second", r, errno);
        two[1].iov_len = 4;
        errno = 0; r = syscall(SYS_readv, dir, two, 2); report("readv-dir-wrap-second", r, errno);
        two[0].iov_base = wrap8; two[1].iov_base = area; two[1].iov_len = SIZE_MAX;
        errno = 0; r = syscall(SYS_writev, null, two, 2); report("writev-wrap-then-badlen", r, errno);
        return 0;
    }
    if (!strcmp(c, "getcwd-low8")) {
        errno = 0; r = syscall(SYS_getcwd, low, 4096);
        report(c, r, errno); return 0;
    }

    return 3;
}
