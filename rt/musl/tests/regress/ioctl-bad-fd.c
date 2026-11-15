/* RTM003, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#include <errno.h>
#include <sys/ioctl.h>
int main(void) { int n; return !(ioctl(-1, FIONREAD, &n) == -1 && errno == EBADF); }
