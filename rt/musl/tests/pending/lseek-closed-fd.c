#include <errno.h>
#include <unistd.h>
int main(void) { close(0); return !(lseek(0, 0, SEEK_SET) == -1 && errno == EBADF); }
