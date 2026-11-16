#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    struct stat st; char b[8]={0};
    CHECK(umask(0027)==0022);
    int fd=open("one",O_CREAT|O_EXCL|O_RDWR|O_CLOEXEC,0666); CHECK(fd==3);
    CHECK(fstat(fd,&st)==0 && (st.st_mode&0777)==0640);
    CHECK((fcntl(fd,F_GETFL)&(O_CREAT|O_EXCL|O_TRUNC|O_CLOEXEC))==0);
    CHECK(fcntl(fd,F_GETFD)==FD_CLOEXEC);
    CHECK(dup2(fd,fd)==fd && fcntl(fd,F_GETFD)==FD_CLOEXEC);
    CHECK(write(fd,"abc",3)==3 && close(fd)==0);
    fd=open("one",O_RDONLY|O_TRUNC); CHECK(fd==3 && fstat(fd,&st)==0 && st.st_size==0);
    CHECK(close(fd)==0);
    errno=0; CHECK(open("one/",O_RDONLY)==-1 && errno==ENOTDIR);
    errno=0; CHECK(open("one/../wrong",O_CREAT|O_RDWR,0600)==-1 && errno==ENOTDIR);
    errno=0; CHECK(open("new/",O_CREAT|O_RDWR,0600)==-1 && errno==EISDIR);
    errno=0; CHECK(stat("new",&st)==-1 && errno==ENOENT);
    errno=0; CHECK(open("absent",O_CREAT|O_DIRECTORY,0700)==-1 && errno==EINVAL);
    errno=0; CHECK(stat("absent",&st)==-1 && errno==ENOENT);
    CHECK(unlink("one")==0);
    fd=open("/dev/zero",O_RDONLY); CHECK(fd==3);
    CHECK(pread(fd,b,8,123)==8 && !memcmp(b,"\0\0\0\0\0\0\0\0",8));
    CHECK(lseek(fd,123,SEEK_SET)==0 && close(fd)==0);
    CHECK(mkdir("outer",0700)==0 && mkdir("outer/child",0700)==0);
    CHECK(chdir("outer/child")==0 && rename("../../outer","../../renamed")==0);
    CHECK(chdir("../..")==0 && rmdir("renamed/child")==0 && rmdir("renamed")==0);
    return 0;
}
