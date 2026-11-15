#include <errno.h>
#include <sys/ioctl.h>
int main(void) { int n; return !(ioctl(-1, FIONREAD, &n) == -1 && errno == EBADF); }
