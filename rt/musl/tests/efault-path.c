/* Path-string checks for every implemented system call that takes a path: how the string is copied in
 * (Linux's getname: the range below the top of the user address space, the bytes up to the first NUL or
 * PATH_MAX of them, ENAMETOOLONG for PATH_MAX bytes without one, ENOENT for an empty string) and where that
 * copy falls in each call's order of checks.
 *
 * argv[1] selects a case (or "agree" for the batch of the order, length and alias cases). Optional argv[2]
 * supplies the end-of-memory address on the device for the case that reaches it; natively, partial_edge maps a
 * read/write page followed by a PROT_NONE guard page. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
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

static void report(const char *case_name, const char *label, long ret, int err)
{
    printf("%s %s: ret=%ld errno=%d\n", case_name, label, ret, err);
}

/* Every path call with path p (and, for rename, p as either name). Nothing exists afterwards that did not
 * before: a refused path creates nothing. */
static void every_call(const char *c, const char *p)
{
    struct stat st;
    char buf[16];
    long r;
    errno = 0; r = syscall(SYS_stat, p, &st); report(c, "stat", r, errno);
    errno = 0; r = syscall(SYS_lstat, p, &st); report(c, "lstat", r, errno);
    errno = 0; r = syscall(SYS_newfstatat, AT_FDCWD, p, &st, 0); report(c, "newfstatat", r, errno);
    errno = 0; r = syscall(SYS_access, p, F_OK); report(c, "access", r, errno);
    errno = 0; r = syscall(SYS_faccessat, AT_FDCWD, p, F_OK); report(c, "faccessat", r, errno);
    errno = 0; r = syscall(SYS_open, p, O_RDONLY); report(c, "open", r, errno);
    errno = 0; r = syscall(SYS_open, p, O_CREAT | O_WRONLY, 0644); report(c, "open-creat", r, errno);
    errno = 0; r = syscall(SYS_openat, AT_FDCWD, p, O_RDONLY); report(c, "openat", r, errno);
    errno = 0; r = syscall(SYS_creat, p, 0644); report(c, "creat", r, errno);
    errno = 0; r = syscall(SYS_readlink, p, buf, sizeof buf); report(c, "readlink", r, errno);
    errno = 0; r = syscall(SYS_readlinkat, AT_FDCWD, p, buf, sizeof buf); report(c, "readlinkat", r, errno);
    errno = 0; r = syscall(SYS_truncate, p, 0); report(c, "truncate", r, errno);
    errno = 0; r = syscall(SYS_chdir, p); report(c, "chdir", r, errno);
    errno = 0; r = syscall(SYS_chmod, p, 0644); report(c, "chmod", r, errno);
    errno = 0; r = syscall(SYS_mkdir, p, 0755); report(c, "mkdir", r, errno);
    errno = 0; r = syscall(SYS_mkdirat, AT_FDCWD, p, 0755); report(c, "mkdirat", r, errno);
    errno = 0; r = syscall(SYS_rmdir, p); report(c, "rmdir", r, errno);
    errno = 0; r = syscall(SYS_unlink, p); report(c, "unlink", r, errno);
    errno = 0; r = syscall(SYS_unlinkat, AT_FDCWD, p, 0); report(c, "unlinkat", r, errno);
    errno = 0; r = syscall(SYS_unlinkat, AT_FDCWD, p, AT_REMOVEDIR); report(c, "unlinkat-dir", r, errno);
    errno = 0; r = syscall(SYS_rename, p, "efault_path.file"); report(c, "rename-old", r, errno);
    errno = 0; r = syscall(SYS_rename, "efault_path.file", p); report(c, "rename-new", r, errno);
    errno = 0; r = access("efault_path.file", F_OK); report(c, "file-still-there", r, errno);
}

int main(int argc, char **argv)
{
    if (argc < 2) return 2;
    const char *c = argv[1];
    int file = open("efault_path.file", O_CREAT | O_RDWR | O_TRUNC, 0644);
    if (file < 0) return 90;
    int closed_fd = dup(file);
    if (closed_fd < 0 || close(closed_fd) != 0) return 91;
    const char *low = (const char *)(uintptr_t)8;
    long r;

    if (!strcmp(c, "path-low8")) {
        /* Natively the lowest page is never mapped: the first byte faults. */
        every_call(c, low);
        every_call("path-null", NULL);
        return 0;
    }
    if (!strcmp(c, "path-wrap8")) {
        every_call(c, (const char *)(uintptr_t)-8);
        return 0;
    }
    if (!strcmp(c, "path-above-top")) {
        /* A string starting at the top of the user address space or above it: EFAULT before any byte is read. */
        every_call("path-at-top", (const char *)(uintptr_t)0x7ffffffff000ull);
        every_call("path-page-above-top", (const char *)(uintptr_t)0x800000000000ull);
        return 0;
    }
    int agree = !strcmp(c, "agree");
    if (agree || !strcmp(c, "path-order")) {
        c = "path-order";
        /* Linux checks these arguments before it copies the path; everything else comes after the copy. */
        char buf[16];
        struct stat st;
        errno = 0; r = syscall(SYS_open, low, O_DIRECTORY | O_CREAT, 0644); report(c, "open-flags", r, errno);
        errno = 0; r = syscall(SYS_openat, closed_fd, low, O_DIRECTORY | O_CREAT, 0644);
        report(c, "openat-flags-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_truncate, low, -1l); report(c, "truncate-negative", r, errno);
        errno = 0; r = syscall(SYS_access, low, 8); report(c, "access-mode", r, errno);
        errno = 0; r = syscall(SYS_faccessat, closed_fd, low, 8); report(c, "faccessat-mode-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_readlink, low, buf, 0); report(c, "readlink-size", r, errno);
        errno = 0; r = syscall(SYS_readlinkat, closed_fd, low, buf, 0); report(c, "readlinkat-size-baddirfd", r, errno);
        /* The path before the directory descriptor, and before what the walk finds. */
        errno = 0; r = syscall(SYS_openat, closed_fd, low, O_RDONLY); report(c, "openat-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_openat, file, low, O_RDONLY); report(c, "openat-filedirfd", r, errno);
        errno = 0; r = syscall(SYS_newfstatat, closed_fd, low, &st, 0); report(c, "newfstatat-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_faccessat, closed_fd, low, F_OK); report(c, "faccessat-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_mkdirat, closed_fd, low, 0755); report(c, "mkdirat-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_unlinkat, closed_fd, low, 0); report(c, "unlinkat-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_readlinkat, closed_fd, low, buf, sizeof buf); report(c, "readlinkat-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_stat, low, NULL); report(c, "stat-nullout", r, errno);
        /* rename: the old name's copy, the old name's walk, then the new name's copy. */
        errno = 0; r = syscall(SYS_rename, low, "efault_path.missing/x"); report(c, "rename-old-first", r, errno);
        errno = 0; r = syscall(SYS_rename, "efault_path.missing/x", low); report(c, "rename-walk-before-new", r, errno);
        errno = 0; r = syscall(SYS_rename, "efault_path.missing", low); report(c, "rename-missing-old", r, errno);
        errno = 0; r = syscall(SYS_rename, "", low); report(c, "rename-empty-old", r, errno);
        errno = 0; r = syscall(SYS_rename, "efault_path.file", ""); report(c, "rename-empty-new", r, errno);
        if (!agree) return 0;
    }
    if (agree || !strcmp(c, "path-long")) {
        c = "path-long";
        /* PATH_MAX bytes with their NUL fit; PATH_MAX bytes without one are too long, whatever follows. */
        static char path[PATH_MAX + 2];
        struct stat st;
        memset(path, '/', PATH_MAX - 1);
        path[PATH_MAX - 1] = 0;
        errno = 0; r = syscall(SYS_stat, path, &st); report(c, "slashes-4095", r, errno);
        errno = 0; r = syscall(SYS_openat, closed_fd, path, O_RDONLY); report(c, "slashes-4095-baddirfd", r, errno);
        path[PATH_MAX - 1] = '/';
        path[PATH_MAX] = 0;
        errno = 0; r = syscall(SYS_stat, path, &st); report(c, "slashes-4096", r, errno);
        errno = 0; r = syscall(SYS_openat, closed_fd, path, O_RDONLY); report(c, "slashes-4096-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_open, path, O_DIRECTORY | O_CREAT); report(c, "slashes-4096-flags", r, errno);
        errno = 0; r = syscall(SYS_rename, path, "efault_path.file"); report(c, "rename-old-4096", r, errno);
        errno = 0; r = syscall(SYS_rename, "efault_path.missing/x", path); report(c, "rename-walk-before-4096", r, errno);
        errno = 0; r = syscall(SYS_rename, "efault_path.file", path); report(c, "rename-new-4096", r, errno);
        /* A component of NAME_MAX bytes is looked up; one more is too long, found by the walk. */
        memset(path, 'n', NAME_MAX + 1);
        path[NAME_MAX] = 0;
        errno = 0; r = syscall(SYS_stat, path, &st); report(c, "name-255", r, errno);
        path[NAME_MAX] = 'n';
        path[NAME_MAX + 1] = 0;
        errno = 0; r = syscall(SYS_stat, path, &st); report(c, "name-256", r, errno);
        errno = 0; r = syscall(SYS_openat, closed_fd, path, O_RDONLY); report(c, "name-256-baddirfd", r, errno);
        errno = 0; r = syscall(SYS_stat, "", &st); report(c, "empty", r, errno);
        errno = 0; r = syscall(SYS_openat, closed_fd, "", O_RDONLY); report(c, "empty-baddirfd", r, errno);
        if (!agree) return 0;
    }
    if (agree || !strcmp(c, "path-alias")) {
        c = "path-alias";
        /* The path is copied before the result is written over it. */
        static union { char path[sizeof(struct stat)]; struct stat st; } shared;
        strcpy(shared.path, "efault_path.file");
        errno = 0; r = syscall(SYS_stat, shared.path, &shared.st);
        printf("%s: ret=%ld errno=%d regular=%d\n", c, r, errno, r == 0 && S_ISREG(shared.st.st_mode));
        return 0;
    }
    if (!strcmp(c, "path-at-edge")) {
        /* The first byte lies past the end of device memory (natively a guard page). */
        char *edge = partial_edge(argc, argv);
        struct stat st;
        errno = 0; r = syscall(SYS_stat, edge, &st); report(c, "stat", r, errno);
        return 0;
    }
    return 3;
}
