#include <errno.h>
#include <stdint.h>
#include <sys/uio.h>
int main(void) { struct iovec v[2]={{"X",1},{"Y",SIZE_MAX}}; return !(writev(1,v,2)==-1 && errno==EINVAL); }
