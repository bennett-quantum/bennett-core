/* The working directory is a directory, not a name. Before the fix a relative create after the cwd was
 * removed and remade landed in the new directory, and fchdir to a removed directory failed. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
#define FAILS(call, code) do { errno=0; CHECK((call)==-1 && errno==(code)); } while(0)
static char base[4096], d[4200], e[4200], buffer[4096];
int main(void) {
    struct stat st;
    CHECK(getcwd(base,sizeof base));
    const char *slash=strcmp(base,"/")?"/":""; /* the device starts in the root */
    snprintf(d,sizeof d,"%s%sd",base,slash); snprintf(e,sizeof e,"%s%se",base,slash);
    /* removed and remade under the same name */
    CHECK(mkdir("d",0755)==0 && chdir("d")==0 && rmdir(d)==0 && mkdir(d,0755)==0);
    FAILS(open("x",O_CREAT|O_WRONLY,0644),ENOENT);
    FAILS(mkdir("y",0755),ENOENT);
    errno=0; CHECK(!getcwd(buffer,sizeof buffer) && errno==ENOENT);
    CHECK(chdir(base)==0 && stat("d/x",&st)==-1 && stat("d/y",&st)==-1);
    /* replaced by a rename */
    CHECK(mkdir("e",0755)==0 && chdir("d")==0 && rename(e,d)==0);
    FAILS(open("x",O_CREAT|O_WRONLY,0644),ENOENT);
    CHECK(chdir(base)==0 && stat("d/x",&st)==-1);
    /* followed through a rename */
    CHECK(chdir("d")==0 && rename(d,e)==0);
    int fd=open("z",O_CREAT|O_WRONLY,0644); CHECK(fd>=0 && close(fd)==0);
    CHECK(getcwd(buffer,sizeof buffer) && !strcmp(buffer,e));
    CHECK(chdir(base)==0 && stat("e/z",&st)==0 && unlink("e/z")==0);
    /* fchdir to a removed directory succeeds; relative names then fail */
    int dir=open("e",O_RDONLY|O_DIRECTORY); CHECK(dir>=0 && rmdir("e")==0);
    CHECK(fchdir(dir)==0);
    errno=0; CHECK(!getcwd(buffer,sizeof buffer) && errno==ENOENT);
    FAILS(open("w",O_CREAT|O_WRONLY,0644),ENOENT);
    FAILS(openat(dir,"w",O_CREAT|O_WRONLY,0644),ENOENT);
    CHECK(chdir(base)==0 && stat("w",&st)==-1 && close(dir)==0);
    return 0;
}
