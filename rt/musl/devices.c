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
#include <time.h>
#include "libc.h"
#include "start.h"
#include "vfs.h"
#include "devices.h"
static unsigned inputs_prepared;

static long transfer(long fd,void *p,uint64_t n,int writing,uint64_t offset,int positional) {
    /* Linux's order: a negative position, the descriptor, a seekable file, then the access mode. */
    if(positional && offset>LONG_MAX) return -EINVAL;
    struct bennett_file *file=__bennett_rt_vfs_file(fd);
    if(!file) return -EBADF;
    if(positional && (file->kind==BRT_CONSOLE || file->kind==BRT_ERROR || file->kind==BRT_INPUT)) return -ESPIPE;
    if(writing?!__bennett_rt_writable(file->flags):!__bennett_rt_readable(file->flags)) return -EBADF;
    if(n>LONG_MAX) return -EINVAL;
    if(n && !p) return -EFAULT;
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
/* Fixed clocks 0..11 (10 is unused), and the CPU-time clocks of the caller: Linux encodes a process or thread
 * as ~id>>3 (0 is the caller; the device's only process and thread are 1) and rejects the clock kind 3. */
static int clock_id(long id) {
    if(id>=0) return id<=11 && id!=10;
    long owner=~(id>>3);
    return (id&3)!=3 && (owner==0 || owner==1);
}
/* The CPU-time clocks among the accepted ids: they do not count the time the process sleeps. */
static int cpu_clock(long id) {
    return id<0 || id==CLOCK_PROCESS_CPUTIME_ID || id==CLOCK_THREAD_CPUTIME_ID;
}
/* Whether the n bytes at p lie where a program's objects can: from address 16, where data starts (below it is
 * no object, as natively the lowest pages are never mapped), to the end of start.c's argument vector, the
 * highest object a program can name (the stack grows down from the top of memory and that vector is in the
 * outermost frame; musl records its auxiliary words as libc.auxv). Before musl's start-up has run, or under an
 * entry other than start.c's, only the low bound and the address's wrap are checked. Memory a program can
 * name but has not reached (below the stack pointer) counts as readable here, as a mapped native page does. */
static int readable(const void *p,uint64_t n) {
    uintptr_t address=(uintptr_t)p, top=libc.auxv?(uintptr_t)(libc.auxv+BENNETT_AUXV_WORDS):UINTPTR_MAX;
    return address>=16 && address<=top && n<=top-address;
}
/* Sleeping on the run's clock (rt/musl/README.md, "Sleeping"). Linux's order of checks: the clock id (an
 * unknown id, then a clock without a sleep), the request's address, the request's value, then what the clock's
 * own sleep checks (a CPU-time clock of a thread, of a process the device does not have, the alarm clocks'
 * flags). No signal arrives
 * while the device sleeps, so a sleep is never interrupted and the remaining time is never written. */
static long clock_sleep(long id,long flags,const long *request) {
    int run=id==CLOCK_REALTIME || id==CLOCK_MONOTONIC || id==CLOCK_BOOTTIME || id==CLOCK_TAI;
    int alarm=id==CLOCK_REALTIME_ALARM || id==CLOCK_BOOTTIME_ALARM;
    int cpu=id==CLOCK_PROCESS_CPUTIME_ID;
    int unsupported=id==CLOCK_THREAD_CPUTIME_ID || id==CLOCK_MONOTONIC_RAW || id==CLOCK_REALTIME_COARSE ||
                    id==CLOCK_MONOTONIC_COARSE || (id<0 && (id&7)==3); /* the last: a clock behind a descriptor */
    if(id>=0 && !run && !alarm && !cpu && !unsupported) return -EINVAL;
    if(unsupported) return -EOPNOTSUPP;
    if(!readable(request,2*sizeof(long))) return -EFAULT; /* Linux copies the whole request first */
    if(request[0]<0 || request[1]<0 || request[1]>=1000000000) return -EINVAL;
    if(id<0) {
        /* A process's CPU-time clock: a kind below 3, not a thread's, the caller (0) or its process (1). */
        long owner=~(id>>3);
        if((id&3)==3 || (id&4) || (owner!=0 && owner!=1)) return -EINVAL;
        cpu=1;
    }
    /* The alarm clocks check their flags (a host without a real-time clock device answers EOPNOTSUPP first),
     * then natively give EPERM without the capability to wake the system and a sleep with it: a fact of the
     * host, refused by name. */
    if(alarm && (flags&~TIMER_ABSTIME)) return -EINVAL;
    if(alarm) __builtin_trap();
    uint64_t wanted;
    if(__builtin_mul_overflow((uint64_t)request[0],1000000000u,&wanted) ||
       __builtin_add_overflow(wanted,(uint64_t)request[1],&wanted))
        __builtin_trap(); /* 2^64 ns or more: beyond the clock's range */
    if(flags&TIMER_ABSTIME) {
        uint64_t now=__bennett_clock_now(cpu);
        if(wanted<=now) return 0;
        wanted-=now;
    }
    if(!wanted) return 0;
    /* A process's CPU time does not advance while it sleeps: natively such a sleep never ends. */
    if(cpu) __builtin_trap();
    __bennett_clock_sleep(wanted);
    return 0;
}
long __bennett_device_syscall(long nr,long a,long b,long c,long d,long e,long f) {
    (void)f;
    /* Lazy preparation runs with musl's allocator ready, also before a constructor's first open.
     * The guard and the preparation itself are ordinary memory, undone with the run. */
    if(!inputs_prepared) { inputs_prepared=1; __bennett_input_files(); }
    if(nr==SYS_clock_gettime) {
        if(!clock_id(a)) return -EINVAL;
        if(!b) return -EFAULT;
        uint64_t value=__bennett_clock_read(cpu_clock(a));
        long *ts=(void *)b; ts[0]=value/1000000000; ts[1]=value%1000000000; return 0;
    }
    if(nr==SYS_clock_getres) {
        if(!clock_id(a)) return -EINVAL;
        if(b) { long *ts=(void *)b; ts[0]=0; ts[1]=1; }
        return 0;
    }
    if(nr==SYS_gettimeofday) {
        if(a) {
            uint64_t value=__bennett_clock_read(0);
            long *tv=(void *)a; tv[0]=value/1000000000; tv[1]=(value%1000000000)/1000;
        }
        if(b) { int *tz=(void *)b; tz[0]=tz[1]=0; }
        return 0;
    }
    if(nr==SYS_time) {
        long seconds=__bennett_clock_read(0)/1000000000;
        if(a) *(long *)a=seconds;
        return seconds;
    }
    if(nr==SYS_nanosleep) return clock_sleep(CLOCK_REALTIME,0,(const long *)a);
    if(nr==SYS_clock_nanosleep) return clock_sleep((int)a,(int)b,(const long *)c); /* both are int in Linux */
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
        if(nr==SYS_writev?!__bennett_rt_writable(file->flags):!__bennett_rt_readable(file->flags)) return -EBADF;
        /* The console is checked whole before any effect; elsewhere a bad element ends a partial transfer, as on Linux. */
        if(file->kind==BRT_CONSOLE || file->kind==BRT_ERROR)
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
