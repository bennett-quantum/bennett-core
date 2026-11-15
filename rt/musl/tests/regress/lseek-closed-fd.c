/* RTM002, found by the independent testing pass of the boundary: self-checking, returns 0 natively. */
#include <errno.h>
#include <unistd.h>
int main(void) { close(0); return !(lseek(0, 0, SEEK_SET) == -1 && errno == EBADF); }
