/* A last component ".", ".." or the root is refused by kind and changes nothing (Linux do_rmdir,
 * do_renameat2). Before the fix rmdir(".") and rename(".", x) removed or moved the directory, and
 * rename(d, "/") replaced the root. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>
#define CHECK(x) do { if(!(x)) return __LINE__; } while(0)
#define FAILS(call, code) do { errno=0; CHECK((call)==-1 && errno==(code)); } while(0)
int main(void) {
    struct stat st;
    CHECK(mkdir("d",0755)==0 && mkdir("d/x",0755)==0 && mkdir("e",0755)==0);
    FAILS(rmdir("d/x/."),EINVAL);
    FAILS(rmdir("d/x/./"),EINVAL);
    FAILS(rmdir("d/x/.."),ENOTEMPTY);
    FAILS(unlinkat(AT_FDCWD,"d/x/.",AT_REMOVEDIR),EINVAL);
    FAILS(unlink("d/x/."),EISDIR);
    FAILS(rmdir("/"),EBUSY);
    FAILS(rename("d/x/.","y"),EBUSY);
    FAILS(rename("d/x/..","y"),EBUSY);
    FAILS(rename("e","d/x/."),EBUSY);
    FAILS(rename("e",".."),EBUSY);
    FAILS(rename("e","/"),EBUSY);
    FAILS(rename("/","e"),EBUSY);
    FAILS(rename("missing/.","y"),ENOENT);
    FAILS(mkdir("d/x/.",0755),EEXIST);
    CHECK(chdir("d/x")==0);
    FAILS(rmdir("."),EINVAL);
    FAILS(rename(".","../moved"),EBUSY);
    CHECK(chdir("../..")==0);
    CHECK(stat("d/x",&st)==0 && S_ISDIR(st.st_mode) && stat("e",&st)==0 && stat("y",&st)==-1);
    CHECK(stat("/",&st)==0 && S_ISDIR(st.st_mode) && st.st_nlink>2);
    CHECK(rmdir("d/x")==0 && rmdir("d")==0 && rmdir("e")==0);
    return 0;
}
