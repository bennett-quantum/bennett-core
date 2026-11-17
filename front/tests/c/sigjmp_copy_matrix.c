/* x86-64 glibc/musl comparison of copied sigjmp_bufs. All saving frames remain live;
 * changed control state is static, so no indeterminate automatic local is observed.
 * Partial copies splice initialized environments at the machine-record/mask boundary
 * shared by these two libcs (64 bytes), or replace just the 8-byte kernel signal mask.
 * Buffer encodings themselves are never compared: the device's record is different. */
#define _GNU_SOURCE
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct holder { sigjmp_buf env; };
union carrier { struct holder holder; unsigned char bytes[sizeof(struct holder)]; };
static struct holder original, copy;
static union carrier transit, onward;
static int method, save, other_save, rewrite, target, changed, landed, value;
static const int saves[] = {0, 1, -7};
enum { METHODS = 8 };
_Static_assert(sizeof(sigjmp_buf) >= 80, "x86-64 jump environment has a mask at byte 72");

static void set_mask(int bits) {
    sigset_t mask;
    sigemptyset(&mask);
    if (bits & 1) sigaddset(&mask, SIGUSR1);
    if (bits & 2) sigaddset(&mask, SIGUSR2);
    if (sigprocmask(SIG_SETMASK, &mask, 0)) _Exit(90);
}

static int get_mask(void) {
    sigset_t mask;
    if (sigprocmask(SIG_BLOCK, 0, &mask)) _Exit(91);
    return sigismember(&mask, SIGUSR1) | (sigismember(&mask, SIGUSR2) << 1);
}

static void transfer(void) {
    unsigned char *dst = (unsigned char *)&copy;
    const unsigned char *src = (const unsigned char *)&original;
    switch (method) {
    case 0: memcpy(&copy, &original, sizeof copy); break;
    case 1: copy = original; break;
    case 2:
        transit.holder = original;
        onward = transit;
        copy = onward.holder;
        break;
    case 3:
        memcpy(transit.bytes, &original, sizeof original);
        onward = transit;
        memcpy(&copy, onward.bytes, sizeof copy);
        break;
    case 4: memcpy(dst, src, 64); break; /* machine record only; destination mask */
    case 5: memcpy(dst, src, 72); break; /* machine record and flag; destination mask */
    case 6: memcpy(dst + 72, src + 72, 8); break; /* kernel mask only */
    case 7: /* full copy in pieces, including splits inside the record, flag and mask */
        memcpy(dst, src, 31);
        memcpy(dst + 31, src + 31, 34);
        memcpy(dst + 65, src + 65, 10);
        memcpy(dst + 75, src + 75, sizeof copy - 75);
        break;
    }
}

__attribute__((noinline)) static void scenario(void) {
    /* Clear old contents so musl savemask=0 cannot inherit mask bytes from another row. */
    memset(&original, 0, sizeof original);
    memset(&copy, 0, sizeof copy);
    set_mask(1);
    switch (sigsetjmp(original.env, saves[save])) {
    case 0: break;
    case 9: landed = 0; goto landing;
    default: landed = -1; goto landing;
    }
    set_mask(2);
    switch (sigsetjmp(copy.env, other_save)) {
    case 0: break;
    case 9: landed = 1; goto landing;
    default: landed = -1; goto landing;
    }
    transfer();
    if (rewrite) {
        set_mask(3);
        switch (sigsetjmp(original.env, rewrite == 1 ? 0 : 1)) {
        case 0: break;
        case 9: landed = 2; goto landing;
        default: landed = -1; goto landing;
        }
    }
    set_mask(changed);
    siglongjmp(target ? copy.env : original.env, 9);
landing:
    value = get_mask();
}

int main(void) {
    for (method = 0; method < METHODS; ++method)
    for (save = 0; save < 3; ++save)
    for (other_save = 0; other_save < 2; ++other_save)
    for (rewrite = 0; rewrite < 3; ++rewrite)
    for (target = 0; target < 2; ++target)
    for (changed = 0; changed < 4; changed += 3) {
        scenario();
        printf("%d %d %d %d %d %d: %d %d\n", method, saves[save], other_save,
               rewrite, target, changed, landed, value);
    }
    set_mask(0);
    return 0;
}
