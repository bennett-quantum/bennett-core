/* SEEK_DATA and SEEK_HOLE over a sparse file, as ext4 and tmpfs with 4 KiB blocks report them.
 * Before the fix both gave EINVAL. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    int fd=open("sparse",O_CREAT|O_RDWR|O_TRUNC,0644); CHECK(fd>=0);
    CHECK(pwrite(fd,"a",1,5000)==1 && pwrite(fd,"b",1,20000)==1 && pwrite(fd,"c",1,4095)==1 && ftruncate(fd,30000)==0);
    static const long at[]={-1,0,100,4096,8191,8192,12287,16384,20479,20480,29999,30000,40000};
    static const long data[]={-1,0,100,4096,8191,16384,16384,16384,20479,-1,-1,-1,-1};
    static const long hole[]={-1,8192,8192,8192,8192,8192,12287,20480,20480,20480,29999,-1,-1};
    for(unsigned i=0;i<sizeof at/sizeof *at;i++) {
        errno=0; CHECK(lseek(fd,at[i],SEEK_DATA)==data[i] && (data[i]>=0 || errno==ENXIO));
        errno=0; CHECK(lseek(fd,at[i],SEEK_HOLE)==hole[i] && (hole[i]>=0 || errno==ENXIO));
    }
    CHECK(ftruncate(fd,8192)==0 && lseek(fd,8000,SEEK_HOLE)==8192 && lseek(fd,0,SEEK_CUR)==8192);
    int dense=open("dense",O_CREAT|O_RDWR|O_TRUNC,0644); CHECK(dense>=0 && write(dense,"hello",5)==5);
    CHECK(lseek(dense,0,SEEK_DATA)==0 && lseek(dense,1,SEEK_HOLE)==5);
    CHECK(close(fd)==0 && close(dense)==0 && unlink("sparse")==0 && unlink("dense")==0);
    return 0;
}
