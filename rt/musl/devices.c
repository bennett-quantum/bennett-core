/* Adapted from bennett-runtimes src/musl/syscall.c; see CARRIED.md.
 * Ordinary libc device/descriptor dispatch; no operation reaches host Linux. */
#define _GNU_SOURCE
#include <stdint.h>
#include <stddef.h>
#include <limits.h>
#include <errno.h>
#include <sys/syscall.h>
#include <fcntl.h>
#include <sys/uio.h>
#include "vfs.h"
#include "devices.h"
static unsigned inputs_prepared;

static long transfer(long fd,void *p,uint64_t n,int writing,uint64_t offset,int positional) {
    struct bennett_file *file=__bennett_rt_vfs_file(fd);
    if(!file) return -EBADF;
    if((file->flags&O_PATH) || (writing?(file->flags&O_ACCMODE)==O_RDONLY:(file->flags&O_ACCMODE)==O_WRONLY)) return -EBADF;
    if(n>LONG_MAX) return -EINVAL;
    if(n && !p) return -EFAULT;
    if(positional && (file->kind==BRT_CONSOLE || file->kind==BRT_ERROR || file->kind==BRT_INPUT)) return -ESPIPE;
    if(positional && offset>LONG_MAX) return -EINVAL;
    if(file->kind==BRT_CONSOLE || file->kind==BRT_ERROR) {
        if(file->kind==BRT_ERROR && n) __builtin_trap();
        for(uint64_t i=0;i<n;++i) __bennett_effect(((const char *)p)[i]);
        return n;
    }
    if(file->kind==BRT_ENTROPY) {
        if(writing) __builtin_trap();
        __bennett_entropy_read(p,n); return n;
    }
    if(file->kind==BRT_INPUT) {
        return 0; /* empty stdin */
    }
    return __bennett_rt_vfs_io(file,p,n,writing,offset,positional);
}
long __bennett_device_syscall(long nr,long a,long b,long c,long d,long e,long f) {
    (void)f;
    /* Lazy preparation runs with musl's allocator ready, also before a constructor's first open.
     * The guard and the preparation itself are ordinary memory, undone with the run. */
    if(!inputs_prepared) { inputs_prepared=1; __bennett_input_files(); }
    if(nr==SYS_clock_gettime) {
        if(a<0 || a>11 || a==10) return -EINVAL;
        if(!b) return -EFAULT;
        uint64_t value=__bennett_clock_read();
        long *ts=(void *)b; ts[0]=value/1000000000; ts[1]=value%1000000000; return 0;
    }
    if(nr==SYS_clock_getres) {
        if(a<0 || a>11 || a==10) return -EINVAL;
        if(b) { long *ts=(void *)b; ts[0]=0; ts[1]=1; }
        return 0;
    }
    if(nr==SYS_gettimeofday) {
        if(a) {
            uint64_t value=__bennett_clock_read();
            long *tv=(void *)a; tv[0]=value/1000000000; tv[1]=(value%1000000000)/1000;
        }
        if(b) { int *tz=(void *)b; tz[0]=tz[1]=0; }
        return 0;
    }
    if(nr==SYS_time) {
        long seconds=__bennett_clock_read()/1000000000;
        if(a) *(long *)a=seconds;
        return seconds;
    }
    if(nr==SYS_getrandom) {
        if((unsigned long)b>LONG_MAX || (c&~7L) || ((c&6)==6)) return -EINVAL;
        if(b && !a) return -EFAULT;
        __bennett_entropy_read((void *)a,b); return b;
    }
    if(nr==SYS_open || nr==SYS_openat) return nr==SYS_open?__bennett_rt_vfs_open((void *)a,b,c,AT_FDCWD):__bennett_rt_vfs_open((void *)b,c,d,a);
    if(nr==SYS_creat) return __bennett_rt_vfs_open((void *)a,O_CREAT|O_WRONLY|O_TRUNC,b,AT_FDCWD);
    if(nr==SYS_close) return __bennett_rt_vfs_close(a);
    if(nr==SYS_dup) return __bennett_rt_vfs_dup(a,0,0,0);
    if(nr==SYS_dup2) return __bennett_rt_vfs_dup(a,b,1,0);
    if(nr==SYS_dup3) {
        if(a==b || (c&~O_CLOEXEC)) return -EINVAL;
        return __bennett_rt_vfs_dup(a,b,1,!!(c&O_CLOEXEC));
    }
    if(nr==SYS_read || nr==SYS_write || nr==SYS_pread64 || nr==SYS_pwrite64) return transfer(a,(void *)b,c,nr==SYS_write || nr==SYS_pwrite64,d,nr==SYS_pread64 || nr==SYS_pwrite64);
    if(nr==SYS_readv || nr==SYS_writev) {
        if(!__bennett_rt_vfs_file(a)) return -EBADF;
        if((unsigned long)c>1024) return -EINVAL;
        if(c && !b) return -EFAULT;
        struct iovec *v=(void *)b; uint64_t total=0;
        for(long i=0;i<c;++i) {
            if(v[i].iov_len>LONG_MAX-total) return -EINVAL;
            total+=v[i].iov_len;
        }
        struct bennett_file *file=__bennett_rt_vfs_file(a);
        if((file->flags&O_PATH) || (nr==SYS_writev?(file->flags&O_ACCMODE)==O_RDONLY:(file->flags&O_ACCMODE)==O_WRONLY)) return -EBADF;
        for(long i=0;i<c;++i) if(v[i].iov_len && !v[i].iov_base) return -EFAULT;
        if(file->kind==BRT_ERROR && nr==SYS_writev && total) __builtin_trap();
        if(file->kind==BRT_ENTROPY && nr==SYS_readv) __bennett_entropy_check(total);
        uint64_t done=0;
        for(long i=0;i<c;++i) if(v[i].iov_len) {
            long n=transfer(a,v[i].iov_base,v[i].iov_len,nr==SYS_writev,0,0);
            if(n<0) return done?(long)done:n;
            done+=n; if((uint64_t)n<v[i].iov_len) break;
        }
        return done;
    }
    if(nr==SYS_lseek || nr==SYS_ioctl) {
        struct bennett_file *file=__bennett_rt_vfs_file(a); if(!file) return -EBADF;
        if(nr==SYS_lseek) {
            return __bennett_rt_vfs_seek(file,b,c,0);
        }
        return -ENOTTY;
    }
    if(nr==SYS_readlink || nr==SYS_readlinkat || nr==SYS_stat || nr==SYS_fstat || nr==SYS_lstat || nr==SYS_access || nr==SYS_fcntl || nr==SYS_fsync || nr==SYS_fdatasync || nr==SYS_truncate || nr==SYS_ftruncate ||
       nr==SYS_getcwd || nr==SYS_chdir || nr==SYS_fchdir || nr==SYS_rename || nr==SYS_mkdir || nr==SYS_rmdir || nr==SYS_unlink || nr==SYS_chmod || nr==SYS_umask ||
       nr==SYS_mkdirat || nr==SYS_newfstatat || nr==SYS_unlinkat || nr==SYS_faccessat) return __bennett_rt_vfs_meta(nr,a,b,c,d,e);
    __builtin_trap();
}
