/* Errors where Linux checks one argument before another, and calls Linux accepts. Before the fix
 * fsync of a directory, an O_ACCMODE open and the orders below differed from native. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
#define FAILS(call, code) do { errno=0; CHECK((call)==-1 && errno==(code)); } while(0)
int main(void) {
    struct stat st; char b[4];
    int fd=open("file",O_CREAT|O_RDWR,0644); CHECK(fd>=0 && write(fd,"abc",3)==3 && close(fd)==0);
    CHECK(mkdir("d",0755)==0);
    int dir=open("d",O_RDONLY|O_DIRECTORY); CHECK(dir>=0);
    CHECK(fsync(dir)==0 && fdatasync(dir)==0);
    int ro=open("file",O_RDONLY); CHECK(ro>=0);
    FAILS(ftruncate(ro,0),EINVAL);
    FAILS(ftruncate(dir,0),EINVAL);
    FAILS(ftruncate(99,-1),EINVAL);
    FAILS(ftruncate(ro,-1),EINVAL);
    FAILS(truncate("missing",-1),EINVAL);
    FAILS(truncate("/dev/null",0),EINVAL);
    FAILS(truncate("d",0),EISDIR);
    FAILS(access("missing",8),EINVAL);
    FAILS(pread(99,b,1,-1),EINVAL);
    FAILS(pwrite(ro,b,1,-1),EINVAL);
    CHECK(fstat(ro,&st)==0 && st.st_size==3);
    /* O_ACCMODE opens for neither reading nor writing */
    int neither=open("file",O_ACCMODE); CHECK(neither>=0);
    CHECK((fcntl(neither,F_GETFL)&O_ACCMODE)==O_ACCMODE);
    FAILS(read(neither,b,1),EBADF);
    FAILS(write(neither,b,1),EBADF);
    FAILS(ftruncate(neither,0),EINVAL);
    FAILS(open("d",O_ACCMODE),EISDIR);
    CHECK(close(neither)==0 && close(ro)==0 && close(dir)==0);
    CHECK(stat("file",&st)==0 && st.st_size==3 && unlink("file")==0 && rmdir("d")==0);
    return 0;
}
