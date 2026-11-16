#include <fcntl.h>
#include <unistd.h>
#include <string.h>
static int ready;
__attribute__((constructor)) static void inspect_input(void) {
    char b[5]; int fd=open("fixtures/input",O_RDONLY);
    ready=fd==3 && read(fd,b,5)==5 && !memcmp(b,"a\0b\377c",5) && close(fd)==0;
}
int main(void) {
    if(!ready) return 3;
    char b[8]={0};
    int fd=open("fixtures/input",O_RDWR);
    if(fd!=3 || read(fd,b,8)!=5 || memcmp(b,"a\0b\377c",5)) return 1;
    if(pwrite(fd,"X",1,2)!=1 || pread(fd,b,5,0)!=5 || memcmp(b,"a\0X\377c",5)) return 2;
    return close(fd);
}
