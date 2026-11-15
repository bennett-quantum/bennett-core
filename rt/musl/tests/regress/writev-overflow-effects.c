/* RTM006, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#include <errno.h>
#include <stdint.h>
#include <sys/uio.h>
int main(void) { struct iovec v[2]={{"X",1},{"Y",SIZE_MAX}}; return !(writev(1,v,2)==-1 && errno==EINVAL); }
