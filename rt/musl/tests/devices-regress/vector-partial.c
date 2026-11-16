/* A vector with a bad element after a good one transfers the good part, as Linux does for files.
 * Before the fix the whole call failed with EFAULT and nothing moved. */
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    int fd=open("v",O_CREAT|O_RDWR|O_TRUNC,0644); CHECK(fd>=0 && write(fd,"abcdef",6)==6);
    char x[2]={0}; struct iovec in[2]={{x,2},{0,3}};
    CHECK(lseek(fd,1,SEEK_SET)==1 && readv(fd,in,2)==2 && !memcmp(x,"bc",2) && lseek(fd,0,SEEK_CUR)==3);
    struct iovec out[2]={{"XY",2},{0,3}};
    CHECK(writev(fd,out,2)==2 && lseek(fd,0,SEEK_CUR)==5);
    char all[8]={0}; CHECK(pread(fd,all,8,0)==6 && !memcmp(all,"abcXYf",6));
    CHECK(close(fd)==0 && unlink("v")==0);
    return 0;
}
