#include <errno.h>
#include <sys/uio.h>
int main(void) { return !(writev(-1, 0, 0) == -1 && errno == EBADF); }
