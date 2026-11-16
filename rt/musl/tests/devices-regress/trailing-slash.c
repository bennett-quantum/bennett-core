/* Trailing slashes and directories, in Linux's order of checks. Before the fix rename(file, "new/")
 * moved the file, open(directory, O_RDONLY|O_TRUNC) succeeded, and the errors below differed. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
#define FAILS(call, code) do { errno=0; CHECK((call)==-1 && errno==(code)); } while(0)
int main(void) {
    struct stat st;
    int fd=open("file",O_CREAT|O_RDWR,0644); CHECK(fd>=0 && close(fd)==0);
    CHECK(mkdir("d",0755)==0);
    FAILS(rename("file","new/"),ENOTDIR);
    FAILS(rename("file/","new"),ENOTDIR);
    CHECK(stat("file",&st)==0 && stat("new",&st)==-1);
    FAILS(rename("file","."),EBUSY);
    FAILS(rename("file","d/."),EBUSY);
    FAILS(open("missing/f/",O_CREAT|O_WRONLY,0644),ENOENT);
    FAILS(open("file/x/",O_CREAT|O_WRONLY,0644),ENOTDIR);
    FAILS(open("file/",O_CREAT|O_WRONLY,0644),EISDIR);
    FAILS(open("new/",O_CREAT|O_WRONLY,0644),EISDIR);
    FAILS(mkdir("file/",0755),EEXIST);
    FAILS(mkdir("d/",0755),EEXIST);
    FAILS(open("d",O_RDONLY|O_TRUNC),EISDIR);
    FAILS(open("d",O_RDONLY|O_CREAT,0644),EISDIR);
    CHECK(mkdir("made/",0755)==0 && rename("made/","moved/")==0 && stat("moved",&st)==0 && S_ISDIR(st.st_mode));
    CHECK(rmdir("moved")==0 && rmdir("d")==0 && unlink("file")==0);
    return 0;
}
