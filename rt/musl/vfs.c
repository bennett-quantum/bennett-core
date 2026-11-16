/* Adapted from bennett-runtimes src/musl/vfs.c; see CARRIED.md.
 * A data-image filesystem: namespace, inodes, sparse pages and descriptors
 * are ordinary program data. The musl arena bounds allocations; no host files. */
#define _GNU_SOURCE
#include "vfs.h"
#include "devices.h"
#include <errno.h>
#include <sys/syscall.h>
#include <fcntl.h>
#include <limits.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* Exhausting the bounded runtime arena is a named fault until syscall-wide rollback exists. */
static void *checked_malloc(size_t n) { void *p=malloc(n); if(!p) __builtin_trap(); return p; }
static void *checked_calloc(size_t n,size_t size) { void *p=calloc(n,size); if(!p) __builtin_trap(); return p; }

struct page { uint64_t number; struct page *next; unsigned char bytes[4096]; };
struct bennett_inode {
    enum bennett_file_kind kind;
    unsigned mode;
    uint64_t number, size, links;
    struct page *pages;
};
struct name { char *path; struct bennett_inode *inode; struct name *next; };
struct descriptor { long fd; int cloexec; struct bennett_file *file; struct descriptor *next; };
static struct bennett_inode initial_inodes[] = {
    {BRT_DIRECTORY,S_IFDIR|0755,1,0,4,0}, {BRT_DIRECTORY,S_IFDIR|0755,2,0,2,0},
    {BRT_ENTROPY,S_IFCHR|0666,3,0,1,0}, {BRT_ENTROPY,S_IFCHR|0666,4,0,1,0},
    {BRT_NULL,S_IFCHR|0666,5,0,1,0}, {BRT_ZERO,S_IFCHR|0666,6,0,1,0},
    {BRT_CONSOLE,S_IFCHR|0666,9,0,1,0}, {BRT_DIRECTORY,S_IFDIR|01777,10,0,2,0}
};
static struct name initial_names[8] = {
    {"/",initial_inodes,&initial_names[1]}, {"/dev",initial_inodes+1,&initial_names[2]},
    {"/dev/random",initial_inodes+2,&initial_names[3]}, {"/dev/urandom",initial_inodes+3,&initial_names[4]},
    {"/dev/null",initial_inodes+4,&initial_names[5]}, {"/dev/zero",initial_inodes+5,&initial_names[6]},
    {"/dev/console",initial_inodes+6,&initial_names[7]}, {"/tmp",initial_inodes+7,0}
};
static struct name *names=initial_names;
static uint64_t next_inode=11;
static unsigned creation_mask=022;
/* The working directory is an inode, as on Linux: it follows renames and, once removed, names nothing. */
static struct bennett_inode *cwd=initial_inodes;
static struct bennett_file standard_files[3] = {
    {BRT_INPUT,O_RDONLY,0,1,0}, {BRT_CONSOLE,O_WRONLY,0,1,0}, {BRT_ERROR,O_WRONLY,0,1,0}
};
static struct descriptor standard_fds[3] = {
    {0,0,standard_files,&standard_fds[1]}, {1,0,standard_files+1,&standard_fds[2]},
    {2,0,standard_files+2,0}
};
static struct descriptor *fds=standard_fds;

static struct descriptor *descriptor(long fd) {
    for(struct descriptor *d=fds;d;d=d->next) if(d->fd==fd) return d;
    return 0;
}
struct bennett_file *__bennett_rt_vfs_file(long fd) {
    struct descriptor *d=descriptor(fd); return d?d->file:0;
}
static long install(struct bennett_file *f,long start,int exact,int cloexec) {
    if(start<0 || start>INT_MAX) return -EINVAL;
    long id=start;
    if(!exact) while(__bennett_rt_vfs_file(id)) { if(id==INT_MAX) return -EMFILE; ++id; }
    struct descriptor *d=descriptor(id);
    if(!d) { d=checked_malloc(sizeof *d); *d=(struct descriptor){id,0,0,fds}; fds=d; }
    if(d->file) --d->file->refs;
    d->file=f; d->cloexec=cloexec; ++f->refs;
    return id;
}
long __bennett_rt_vfs_close(long fd) {
    struct descriptor *d=descriptor(fd);
    if(!d || !d->file) return -EBADF;
    --d->file->refs; d->file=0; return 0;
}
long __bennett_rt_vfs_dup(long old,long start,int exact,int cloexec) {
    struct bennett_file *f=__bennett_rt_vfs_file(old);
    if(!f) return -EBADF;
    if(exact && (start<0 || start>INT_MAX)) return -EBADF;
    if(exact && old==start) return old;
    return install(f,start,exact,cloexec);
}
static struct name *lookup(const char *path) {
    for(struct name *n=names;n;n=n->next) if(n->inode && !strcmp(path,n->path)) return n;
    return 0;
}
/* The live name of an inode, or none once it is removed. */
static const char *inode_path(struct bennett_inode *i) {
    for(struct name *n=names;n;n=n->next) if(n->inode==i) return n->path;
    return 0;
}
/* How Linux classifies a path's last component: 1 for ".", 2 for "..", 3 for the root alone, 0 otherwise. */
static int last_type(const char *path) {
    size_t end=strlen(path);
    while(end && path[end-1]=='/') --end;
    if(!end) return *path=='/'?3:0;
    size_t begin=end; while(begin && path[begin-1]!='/') --begin;
    if(end-begin==1 && path[begin]=='.') return 1;
    if(end-begin==2 && path[begin]=='.' && path[begin+1]=='.') return 2;
    return 0;
}
static int trailing_slash(const char *path) { size_t n=strlen(path); return n && path[n-1]=='/'; }
/* Linux's limits: a path of PATH_MAX bytes or more and a component of more than NAME_MAX bytes are too long.
 * With `tail`, a trailing slash after an existing non-directory is ENOTDIR (callers that order it otherwise pass 0). */
static char *canonical(const char *path,long at,int *error,int tail) {
    if(!path) { *error=EFAULT; return 0; }
    if(!*path) { *error=ENOENT; return 0; }
    if(strnlen(path,PATH_MAX)>=PATH_MAX) { *error=ENAMETOOLONG; return 0; }
    const char *begin_path=path;
    const char *base="/";
    if(*path!='/') {
        struct bennett_inode *directory=cwd;
        if(at!=AT_FDCWD) {
            struct bennett_file *f=__bennett_rt_vfs_file(at);
            if(!f) { *error=EBADF; return 0; }
            if(f->kind!=BRT_DIRECTORY) { *error=ENOTDIR; return 0; }
            directory=f->inode;
        }
        base=inode_path(directory);
        if(!base) { *error=ENOENT; return 0; }
    }
    size_t a=*path=='/'?0:strlen(base), b=strlen(path);
    if(b>SIZE_MAX-a-2) { *error=ENAMETOOLONG; return 0; }
    char *result=checked_malloc(a+b+2); size_t used=1; result[0]='/';
    if(a>1) { memcpy(result,base,a); used=a; }
    while(*path) {
        while(*path=='/') ++path;
        const char *begin=path; while(*path && *path!='/') ++path;
        size_t len=path-begin;
        if(len>NAME_MAX) { *error=ENAMETOOLONG; free(result); return 0; }
        if(!len || (len==1 && begin[0]=='.')) continue;
        if(len==2 && begin[0]=='.' && begin[1]=='.') {
            while(used>1 && result[used-1]!='/') --used;
            if(used>1) --used;
        } else {
            if(used>1) result[used++]='/';
            memcpy(result+used,begin,len); used+=len;
        }
        const char *rest=path; while(*rest=='/') ++rest;
        if(*rest) {
            result[used]=0; struct name *n=lookup(result);
            if(!n || n->inode->kind!=BRT_DIRECTORY) {
                *error=n?ENOTDIR:ENOENT; free(result); return 0;
            }
        }
    }
    result[used]=0;
    if(tail && path>begin_path && path[-1]=='/') {
        struct name *n=lookup(result);
        if(n && n->inode->kind!=BRT_DIRECTORY) { *error=ENOTDIR; free(result); return 0; }
    }
    return result;
}
static struct bennett_inode *parent_inode(char *path) {
    char *slash=strrchr(path,'/'); if(!slash) return 0;
    char old=slash[1]; if(slash==path) slash[1]=0; else *slash=0;
    struct name *p=lookup(path);
    if(slash==path) slash[1]=old; else *slash='/';
    return p?p->inode:0;
}
static int parent_directory(char *path) {
    struct bennett_inode *p=parent_inode(path);
    return !p?-ENOENT:p->kind!=BRT_DIRECTORY?-ENOTDIR:0;
}
static struct name *create_name(char *path,unsigned mode,enum bennett_file_kind kind) {
    struct bennett_inode *i=checked_malloc(sizeof *i); *i=(struct bennett_inode){kind,mode,next_inode++,0,kind==BRT_DIRECTORY?2:1,0};
    if(kind==BRT_DIRECTORY) ++parent_inode(path)->links;
    struct name *n=checked_malloc(sizeof *n); *n=(struct name){path,i,names}; names=n; return n;
}
static void truncate_inode(struct bennett_inode *i,uint64_t size) {
    if(size<i->size) {
        struct page **link=&i->pages;
        while(*link) {
            struct page *p=*link;
            if(p->number>size/4096 || (p->number==size/4096 && !(size%4096))) {
                *link=p->next; free(p);
            } else {
                if(p->number==size/4096) memset(p->bytes+size%4096,0,4096-size%4096);
                link=&p->next;
            }
        }
    }
    i->size=size;
}
long __bennett_rt_vfs_open(const char *path,int flags,unsigned mode,long at) {
    if(flags & ~(O_ACCMODE|O_CREAT|O_EXCL|O_TRUNC|O_APPEND|O_NONBLOCK|O_CLOEXEC|O_DIRECTORY|O_NOFOLLOW|O_LARGEFILE|O_NOCTTY|O_SYNC|O_DSYNC|O_TMPFILE)) __builtin_trap();
    /* Flag errors precede the path walk, as in Linux's build_open_flags. O_ACCMODE itself is a valid mode. */
    if((flags&(O_DIRECTORY|O_CREAT))==(O_DIRECTORY|O_CREAT)) return -EINVAL;
    if((flags&(O_TMPFILE&~O_DIRECTORY)) && ((flags&O_TMPFILE)!=O_TMPFILE || (flags&O_ACCMODE)==O_RDONLY)) return -EINVAL;
    mode &= ~creation_mask;
    int error=0; char *full=canonical(path,at,&error,0); if(!full) return -error;
    struct name *n=lookup(full);
    if(trailing_slash(path) && (flags&O_CREAT)) { free(full); return -EISDIR; }
    if(trailing_slash(path) && n && n->inode->kind!=BRT_DIRECTORY) { free(full); return -ENOTDIR; }
    if((flags&O_TMPFILE)==O_TMPFILE) {
        if(!n) { free(full); return -ENOENT; }
        if(n->inode->kind!=BRT_DIRECTORY) { free(full); return -ENOTDIR; }
        free(full);
        struct bennett_inode *i=checked_calloc(1,sizeof *i);
        i->kind=BRT_REGULAR; i->mode=S_IFREG|(mode&07777); i->number=next_inode++;
        struct bennett_file *f=checked_malloc(sizeof *f);
        *f=(struct bennett_file){BRT_REGULAR,(flags&~(O_CREAT|O_EXCL|O_TRUNC|O_CLOEXEC|O_NOCTTY))|O_LARGEFILE,0,0,i}; return install(f,0,0,!!(flags&O_CLOEXEC));
    }
    if(!n) {
        if(!(flags&O_CREAT)) { free(full); return -ENOENT; }
        int r=parent_directory(full); if(r) { free(full); return r; }
        n=create_name(full,S_IFREG|(mode&07777),BRT_REGULAR); full=0;
    } else if((flags&(O_CREAT|O_EXCL))==(O_CREAT|O_EXCL)) { free(full); return -EEXIST; }
    free(full);
    if((flags&O_DIRECTORY) && n->inode->kind!=BRT_DIRECTORY) return -ENOTDIR;
    /* O_TRUNC asks for write access, so a directory refuses it as Linux's may_open does. */
    if(n->inode->kind==BRT_DIRECTORY && ((flags&O_ACCMODE)!=O_RDONLY || (flags&(O_CREAT|O_TRUNC)))) return -EISDIR;
    if((flags&O_TRUNC) && n->inode->kind==BRT_REGULAR)
        truncate_inode(n->inode,0);
    struct bennett_file *f=checked_malloc(sizeof *f);
    *f=(struct bennett_file){n->inode->kind,(flags&~(O_CREAT|O_EXCL|O_TRUNC|O_CLOEXEC|O_NOCTTY))|O_LARGEFILE,0,0,n->inode};
    return install(f,0,0,!!(flags&O_CLOEXEC));
}
long __bennett_rt_vfs_io(struct bennett_file *f,void *buffer,uint64_t n,int writing,uint64_t offset,int positional) {
    if(writing?!__bennett_rt_writable(f->flags):!__bennett_rt_readable(f->flags)) return -EBADF;
    if(f->kind==BRT_DIRECTORY) return -EISDIR;
    if(f->kind==BRT_NULL) return writing?(long)n:0;
    if(f->kind==BRT_ZERO) { if(!writing) memset(buffer,0,n); return n; }
    if(!n) return 0;
    struct bennett_inode *i=f->inode;
    uint64_t pos=positional?offset:f->pos;
    if(writing && (f->flags&O_APPEND)) pos=i->size;
    if(pos>LONG_MAX || n>(uint64_t)LONG_MAX-pos) return -EFBIG;
    if(!writing) { if(pos>=i->size) n=0; else if(n>i->size-pos) n=i->size-pos; }
    unsigned char *bytes=buffer; uint64_t done=0;
    while(done<n) {
        uint64_t number=(pos+done)/4096, index=(pos+done)%4096, count=4096-index;
        if(count>n-done) count=n-done;
        struct page *p=i->pages;
        while(p && p->number!=number) p=p->next;
        if(writing && !p) { p=checked_calloc(1,sizeof *p); p->number=number; p->next=i->pages; i->pages=p; }
        if(writing) memcpy(p->bytes+index,bytes+done,count);
        else if(p) memcpy(bytes+done,p->bytes+index,count);
        else memset(bytes+done,0,count);
        done+=count;
    }
    if(writing && pos+n>i->size) i->size=pos+n;
    if(!positional) f->pos=pos+n;
    return n;
}
long __bennett_rt_vfs_seek(struct bennett_file *f,int64_t offset,int whence,uint64_t block_size) {
    (void)block_size;
    if(f->kind==BRT_NULL || f->kind==BRT_ZERO || f->kind==BRT_ENTROPY) return 0;
    if(f->kind!=BRT_REGULAR && f->kind!=BRT_DIRECTORY) return -ESPIPE;
    if(whence==SEEK_DATA || whence==SEEK_HOLE) {
        if(f->kind!=BRT_REGULAR) __builtin_trap(); /* directory data and holes are not modelled */
        /* Holes are the unwritten 4096-byte pages, as ext4 and tmpfs report them; the end of file is a hole. */
        uint64_t size=f->inode->size, at=(uint64_t)offset, number=at/4096, found;
        if(at>=size) return -ENXIO;
        if(whence==SEEK_DATA) {
            uint64_t first=UINT64_MAX;
            for(struct page *p=f->inode->pages;p;p=p->next) if(p->number>=number && p->number<first) first=p->number;
            if(first==UINT64_MAX || first>=(size+4095)/4096) return -ENXIO;
            found=first==number?at:first*4096;
        } else {
            for(struct page *p=f->inode->pages;p;)
                if(p->number==number) { ++number; p=f->inode->pages; } else p=p->next;
            found=number==at/4096?at:number*4096;
            if(found>size) found=size;
        }
        return f->pos=found;
    }
    if(whence<0 || whence>2) return -EINVAL;
    uint64_t base=whence==0?0:whence==1?f->pos:f->inode->size;
    uint64_t magnitude=offset<0?0-(uint64_t)offset:(uint64_t)offset;
    if(offset<0 ? magnitude>base : magnitude>(uint64_t)LONG_MAX-base) return -EINVAL;
    f->pos=offset<0?base-magnitude:base+magnitude; return f->pos;
}
static long fill_stat(struct bennett_inode *i,struct stat *st) {
    if(!st) return -EFAULT;
    memset(st,0,sizeof *st); st->st_dev=1; st->st_ino=i->number; st->st_nlink=i->links;
    st->st_mode=i->mode; st->st_size=i->size; st->st_blksize=4096;
    for(struct page *p=i->pages;p;p=p->next) st->st_blocks+=8;
    return 0;
}
long __bennett_rt_vfs_meta(long nr,long a,long b,long c,long d,long e) {
    (void)e;
    if(nr==SYS_umask) { unsigned old=creation_mask; creation_mask=a&0777; return old; }
    if(nr==SYS_fstat) {
        struct bennett_file *f=__bennett_rt_vfs_file(a); if(!f) return -EBADF;
        if(f->inode) return fill_stat(f->inode,(void *)b);
        struct bennett_inode device={f->kind,S_IFCHR|0666,0,0,1,0}; return fill_stat(&device,(void *)b);
    }
    if(nr==SYS_fcntl) {
        struct descriptor *fd=descriptor(a); if(!fd || !fd->file) return -EBADF;
        if(b==F_DUPFD || b==F_DUPFD_CLOEXEC) return __bennett_rt_vfs_dup(a,c,0,b==F_DUPFD_CLOEXEC);
        if(b==F_GETFD) return fd->cloexec?FD_CLOEXEC:0;
        if(b==F_SETFD) { fd->cloexec=!!(c&FD_CLOEXEC); return 0; }
        if(b==F_GETFL) return fd->file->flags;
        if(b==F_SETFL) {
            if(c & ~(long)(O_ACCMODE|O_APPEND|O_NONBLOCK|O_LARGEFILE|O_SYNC|O_DSYNC)) __builtin_trap();
            fd->file->flags=(fd->file->flags&~(O_APPEND|O_NONBLOCK))|(c&(O_APPEND|O_NONBLOCK)); return 0;
        }
        __builtin_trap();
    }
    if(nr==SYS_fsync || nr==SYS_fdatasync) {
        struct bennett_file *f=__bennett_rt_vfs_file(a); if(!f) return -EBADF;
        return f->kind==BRT_REGULAR || f->kind==BRT_DIRECTORY?0:-EINVAL;
    }
    if(nr==SYS_ftruncate) {
        /* Linux's order: the length, the descriptor, then a regular file open for writing (else EINVAL). */
        if(b<0) return -EINVAL;
        struct bennett_file *f=__bennett_rt_vfs_file(a); if(!f) return -EBADF;
        if(f->kind!=BRT_REGULAR || !__bennett_rt_writable(f->flags)) return -EINVAL;
        truncate_inode(f->inode,b); return 0;
    }
    if(nr==SYS_getcwd) {
        const char *path=inode_path(cwd); if(!path) return -ENOENT;
        size_t n=strlen(path)+1; if((uint64_t)b<n) return -ERANGE;
        if(!a) return -EFAULT; memcpy((void *)a,path,n); return n;
    }
    if(nr==SYS_fchdir) {
        struct bennett_file *f=__bennett_rt_vfs_file(a); if(!f) return -EBADF;
        if(f->kind!=BRT_DIRECTORY) return -ENOTDIR;
        cwd=f->inode; return 0; /* also a removed directory, as on Linux */
    }
    if((nr==SYS_readlink && c<=0) || (nr==SYS_readlinkat && d<=0)) return -EINVAL;
    if((nr==SYS_newfstatat && (d & ~AT_SYMLINK_NOFOLLOW)) || (nr==SYS_unlinkat && (c & ~AT_REMOVEDIR))) __builtin_trap();
    /* Argument errors Linux reports before the path walk. */
    if(nr==SYS_truncate && b<0) return -EINVAL;
    if((nr==SYS_access && (b&~7L)) || (nr==SYS_faccessat && (c&~7L))) return -EINVAL;
    long at=AT_FDCWD; const char *path=(void *)a;
    if(nr==SYS_readlinkat || nr==SYS_openat || nr==SYS_newfstatat || nr==SYS_faccessat || nr==SYS_mkdirat || nr==SYS_unlinkat) { at=a; path=(void *)b; }
    int creating=nr==SYS_mkdir || nr==SYS_mkdirat, removing=nr==SYS_unlink || nr==SYS_rmdir || nr==SYS_unlinkat;
    int error=0; char *full=canonical(path,at,&error,!creating && nr!=SYS_rename); if(!full) return -error;
    struct name *n=lookup(full); long result=0;
    /* A last component ".", ".." or the root: Linux refuses these by kind before looking the name up. */
    int kind=last_type(path), directory=nr==SYS_rmdir || (nr==SYS_unlinkat && (c&AT_REMOVEDIR));
    char *dest=0;
    if(nr==SYS_rename && !(dest=canonical((void *)b,AT_FDCWD,&error,0))) result=-error;
    else if(nr==SYS_rename && (kind || last_type((void *)b))) result=-EBUSY;
    else if(creating && kind) result=-EEXIST;
    else if(removing && kind) result=!directory?-EISDIR:kind==1?-EINVAL:kind==2?-ENOTEMPTY:-EBUSY;
    else if(creating) {
        if(n) result=-EEXIST;
        else if((result=parent_directory(full))==0) {
            create_name(full,S_IFDIR|((nr==SYS_mkdir?b:c)&07777&~creation_mask),BRT_DIRECTORY); full=0;
        }
    } else if(!n) result=-ENOENT;
    else if(nr==SYS_rename && n->inode->kind!=BRT_DIRECTORY && (trailing_slash(path) || trailing_slash((void *)b))) result=-ENOTDIR;
    else if(nr==SYS_readlink || nr==SYS_readlinkat) result=-EINVAL; /* no symlinks in this namespace yet */
    else if(nr==SYS_stat || nr==SYS_lstat || nr==SYS_newfstatat) result=fill_stat(n->inode,(void *)(nr==SYS_newfstatat?c:b));
    else if(nr==SYS_access || nr==SYS_faccessat) {
        long mode=nr==SYS_access?b:c;
        if((mode&X_OK) && n->inode->kind!=BRT_DIRECTORY && !(n->inode->mode&0111)) result=-EACCES;
    } else if(nr==SYS_truncate) {
        if(n->inode->kind==BRT_DIRECTORY) result=-EISDIR;
        else if(n->inode->kind!=BRT_REGULAR) result=-EINVAL;
        else truncate_inode(n->inode,b);
    } else if(nr==SYS_chdir) {
        if(n->inode->kind!=BRT_DIRECTORY) result=-ENOTDIR; else cwd=n->inode;
    } else if(removing) {
        if(directory && n->inode->kind!=BRT_DIRECTORY) result=-ENOTDIR;
        else if(!directory && n->inode->kind==BRT_DIRECTORY) result=-EISDIR;
        else if(!strcmp(full,"/")) result=-EBUSY;
        else {
            size_t len=strlen(full);
            if(directory) for(struct name *other=names;other;other=other->next)
                if(other->inode && !strncmp(other->path,full,len) && other->path[len]=='/') result=-ENOTEMPTY;
            if(!result) {
                if(directory) { --parent_inode(full)->links; n->inode->links=0; }
                else --n->inode->links;
                n->inode=0;
            }
        }
    } else if(nr==SYS_rename) {
        if(!strcmp(full,dest)) result=0;
        else if(n->inode->kind==BRT_DIRECTORY && !strncmp(dest,full,strlen(full)) && dest[strlen(full)]=='/') result=-EINVAL;
        else if((result=parent_directory(dest))==0) {
            struct name *existing=lookup(dest);
            if(existing && existing->inode->kind==BRT_DIRECTORY && n->inode->kind!=BRT_DIRECTORY) result=-EISDIR;
            else if(existing && existing->inode->kind!=BRT_DIRECTORY && n->inode->kind==BRT_DIRECTORY) result=-ENOTDIR;
            else {
                size_t oldlen=strlen(full);
                if(existing && existing->inode->kind==BRT_DIRECTORY) {
                    size_t len=strlen(dest);
                    for(struct name *child=names;child;child=child->next) if(child->inode &&
                        !strncmp(child->path,dest,len) && child->path[len]=='/') result=-ENOTEMPTY;
                }
                if(result) { free(dest); free(full); return result; }
                if(existing) {
                    if(existing->inode->kind==BRT_DIRECTORY) { --parent_inode(dest)->links; existing->inode->links=0; }
                    else --existing->inode->links;
                    existing->inode=0;
                }
                if(n->inode->kind==BRT_DIRECTORY) {
                    --parent_inode(full)->links; ++parent_inode(dest)->links;
                    for(struct name *child=names;child;child=child->next) if(child->inode &&
                        !strncmp(child->path,full,oldlen) && child->path[oldlen]=='/') {
                        size_t prefix=strlen(dest), tail=strlen(child->path+oldlen)+1;
                        char *updated=checked_malloc(prefix+tail);
                        memcpy(updated,dest,prefix); memcpy(updated+prefix,child->path+oldlen,tail); child->path=updated;
                    }
                }
                n->path=dest; dest=0;
            }
        }
    } else if(nr==SYS_chmod) n->inode->mode=(n->inode->mode&S_IFMT)|(b&07777);
    else result=-ENOSYS;
    free(dest); free(full); (void)d; return result;
}
