#ifndef BENNETT_VFS_H
#define BENNETT_VFS_H
#include <stdint.h>
enum bennett_file_kind { BRT_REGULAR, BRT_DIRECTORY, BRT_CONSOLE, BRT_INPUT,
                         BRT_ENTROPY, BRT_NULL, BRT_ZERO, BRT_ERROR };
struct bennett_inode;
struct bennett_file {
    enum bennett_file_kind kind;
    int flags;
    uint64_t pos, refs;
    struct bennett_inode *inode;
};
struct bennett_file *__bennett_rt_vfs_file(long);
long __bennett_rt_vfs_open(const char *,int,unsigned,long);
long __bennett_rt_vfs_close(long);
long __bennett_rt_vfs_dup(long,long,int,int);
long __bennett_rt_vfs_io(struct bennett_file *,void *,uint64_t,int,uint64_t,int);
long __bennett_rt_vfs_seek(struct bennett_file *,int64_t,int,uint64_t);
long __bennett_rt_vfs_meta(long,long,long,long,long,long);
#endif
