#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <unistd.h>
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
int main(void) {
    char b[32]; struct stat st;
    CHECK(mkdir("data",0750)==0);
    int fd=open("data/image",O_CREAT|O_EXCL|O_RDWR,0640); CHECK(fd==3);
    CHECK(write(fd,"abcdefgh",8)==8);
    CHECK(pwrite(fd,"ignored",0,100)==0);
    CHECK(fstat(fd,&st)==0 && st.st_size==8 && (st.st_mode&0777)==0640);
    int alias=dup(fd); CHECK(alias==4 && lseek(alias,2,SEEK_SET)==2);
    CHECK(read(fd,b,3)==3 && !memcmp(b,"cde",3));
    CHECK(close(alias)==0);
    CHECK(pwrite(fd,"XY",2,1)==2 && lseek(fd,0,SEEK_CUR)==5);
    CHECK(pread(fd,b,8,0)==8 && !memcmp(b,"aXYdefgh",8));
    CHECK(ftruncate(fd,16389)==0 && pread(fd,b,8,16381)==8);
    for(int i=0;i<8;++i) CHECK(!b[i]);
    CHECK(pwrite(fd,"tail",4,16385)==4);
    CHECK(ftruncate(fd,4)==0 && ftruncate(fd,16389)==0 && pread(fd,b,4,16385)==4);
    for(int i=0;i<4;++i) CHECK(!b[i]);
    CHECK(close(fd)==0);
    FILE *f=fopen("data/image","a"); CHECK(f && fseek(f,0,SEEK_SET)==0);
    CHECK(fwrite("end",1,3,f)==3 && fclose(f)==0);
    CHECK(stat("data/image",&st)==0 && st.st_size==16392);
    CHECK(rename("data/image","data/renamed")==0);
    CHECK((fd=open("data/renamed",O_RDONLY))>=3 && unlink("data/renamed")==0);
    CHECK(pread(fd,b,3,16389)==3 && !memcmp(b,"end",3));
    CHECK(close(fd)==0);
    int dir=open("data",O_RDONLY|O_DIRECTORY); CHECK(dir==3);
    fd=openat(dir,"relative",O_CREAT|O_RDWR,0600); CHECK(fd==4);
    struct iovec v[2]={{"12",2},{"345",3}};
    CHECK(writev(fd,v,2)==5 && lseek(fd,0,SEEK_SET)==0);
    v[0]=(struct iovec){b,3}; v[1]=(struct iovec){b+3,9};
    CHECK(readv(fd,v,2)==5 && !memcmp(b,"12345",5));
    CHECK(fcntl(fd,F_SETFD,FD_CLOEXEC)==0 && fcntl(fd,F_GETFD)==FD_CLOEXEC);
    CHECK(dup2(fd,7)==7 && lseek(7,1,SEEK_SET)==1 && read(fd,b,1)==1 && b[0]=='2');
    CHECK(close(7)==0 && close(fd)==0);
    CHECK(unlinkat(dir,"relative",0)==0 && close(dir)==0 && rmdir("data")==0);
    fd=open("/dev/zero",O_RDONLY); CHECK(fd==3 && read(fd,b,32)==32);
    for(int i=0;i<32;++i) CHECK(!b[i]);
    CHECK(close(fd)==0);
    errno=0; CHECK(open("missing",O_RDONLY)==-1 && errno==ENOENT);
    CHECK(close(1)==0);
    fd=open("reused",O_CREAT|O_RDWR,0600); CHECK(fd==1);
    CHECK(write(fd,"R",1)==1 && pread(fd,b,1,0)==1 && b[0]=='R');
    CHECK(close(fd)==0 && unlink("reused")==0);
    return 0;
}
