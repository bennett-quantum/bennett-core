/* Partial copying within x86-64's callee-saved-register area leaves the target
 * continuation intact in glibc and musl. The device instead copies its continuation.
 * Both saves are in the live main frame; no changed automatic local is observed. */
#define _GNU_SOURCE
#include <setjmp.h>
#include <string.h>

static sigjmp_buf original, copy;

int main(void) {
    switch (sigsetjmp(original, 0)) {
    case 0: break;
    default: return 1;
    }
    switch (sigsetjmp(copy, 0)) {
    case 0: break;
    default: return 0;
    }
    memcpy(copy, original, 16);
    siglongjmp(copy, 1);
}
