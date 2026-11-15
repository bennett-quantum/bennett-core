/* RTM001, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#include <errno.h>
#include <sys/uio.h>
int main(void) { return !(writev(-1, 0, 0) == -1 && errno == EBADF); }
