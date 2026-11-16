#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sys/random.h>
#include <sys/uio.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    unsigned char b[300];
    CHECK(getentropy(b,256)==0);
    for(int i=0;i<256;++i) CHECK(b[i]==(unsigned char)(17*i+3));
    errno=0; CHECK(getentropy(b,257)==-1 && errno==EIO);
    int fd=open("/dev/urandom",O_RDONLY); CHECK(fd==3);
    struct iovec v[3]={{b,5},{0,0},{b+5,7}};
    CHECK(readv(fd,v,3)==12 && close(fd)==0);
    for(int i=0;i<12;++i) CHECK(b[i]==(unsigned char)(17*(256+i)+3));
    fd=open("/dev/random",O_RDONLY); CHECK(fd==3 && read(fd,b,3)==3 && close(fd)==0);
    for(int i=0;i<3;++i) CHECK(b[i]==(unsigned char)(17*(268+i)+3));
    return 0;
}
