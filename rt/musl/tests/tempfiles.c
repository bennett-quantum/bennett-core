#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
int main(void) {
    char path[L_tmpnam], b[8]={0}; struct stat st;
    CHECK(tmpnam(path)==path);
    errno=0; CHECK(readlink(path,b,sizeof b)==-1 && errno==ENOENT);
    FILE *f=fopen(path,"w+"); CHECK(f);
    CHECK(fwrite("abc",1,3,f)==3 && fflush(f)==0 && fseek(f,0,SEEK_SET)==0);
    CHECK(fread(b,1,3,f)==3 && !memcmp(b,"abc",3));
    errno=0; CHECK(readlink(path,b,sizeof b)==-1 && errno==EINVAL);
    CHECK(fclose(f)==0 && unlink(path)==0);
    f=tmpfile(); CHECK(f);
    int fd=fileno(f); CHECK(fstat(fd,&st)==0 && st.st_nlink==0);
    CHECK((fcntl(fd,F_GETFL)&(O_CREAT|O_EXCL|O_TRUNC|O_CLOEXEC))==0);
    CHECK(fputs("xyz",f)>=0 && fseek(f,0,SEEK_SET)==0);
    CHECK(fread(b,1,3,f)==3 && !memcmp(b,"xyz",3) && fclose(f)==0);
    return 0;
}
