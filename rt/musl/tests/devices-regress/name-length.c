/* Linux's limits on names: a component of more than NAME_MAX bytes and a path of PATH_MAX bytes or
 * more are too long, after earlier components are found. Before the fix both were accepted. */
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
#define FAILS(call, code) do { errno=0; CHECK((call)==-1 && errno==(code)); } while(0)
static char longest[NAME_MAX+1], over[NAME_MAX+2], nested[NAME_MAX+16], missing[NAME_MAX+16], path[PATH_MAX+1];
int main(void) {
    struct stat st;
    memset(longest,'a',NAME_MAX); memset(over,'b',NAME_MAX+1);
    int fd=open(longest,O_CREAT|O_WRONLY,0644); CHECK(fd>=0 && close(fd)==0 && unlink(longest)==0);
    FAILS(open(over,O_CREAT|O_WRONLY,0644),ENAMETOOLONG);
    FAILS(stat(over,&st),ENAMETOOLONG);
    FAILS(mkdir(over,0755),ENAMETOOLONG);
    memcpy(nested,over,NAME_MAX+1); memcpy(nested+NAME_MAX+1,"/x",3);
    FAILS(stat(nested,&st),ENAMETOOLONG);
    memcpy(missing,"absent/",7); memcpy(missing+7,over,NAME_MAX+1);
    FAILS(stat(missing,&st),ENOENT);
    memset(path,'/',PATH_MAX-1);
    fd=open(path,O_RDONLY|O_DIRECTORY); CHECK(fd>=0 && close(fd)==0);
    path[PATH_MAX-1]='/';
    FAILS(open(path,O_RDONLY|O_DIRECTORY),ENAMETOOLONG);
    return 0;
}
