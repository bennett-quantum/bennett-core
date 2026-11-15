/* The signal mask travels with a jump environment (docs/DESIGN.md, "Non-local jumps"), checked against
 * native on every way of saving, copying and jumping that one frame allows with two buffers and a copy,
 * two visits and two jumps: five call sites (sigsetjmp with a run-time savemask of 0 or 1, with constant 1
 * and constant 0, and setjmp), each visit under its own mask and each jump from its own, by siglongjmp or
 * longjmp. Then sixteen buffers saved at one site, copied, saved again with the opposite savemask and other
 * masks, and jumped through in a chain of copies. Only defined C and POSIX state is observed: the mask after
 * each landing, the landing site and the value; the frame stays live and changed state is volatile. */
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>

enum { KINDS = 5, BUFFERS = 2, COPY = BUFFERS, VISITS = 2, JUMPS = 2, CHOICES = KINDS * BUFFERS * 2 };
static const int signals[] = {SIGUSR1, SIGUSR2, SIGHUP, SIGALRM, SIGTERM};

static void set_mask(int bits) {
    sigset_t set;
    sigemptyset(&set);
    for (int i = 0; i < 5; ++i)
        if (bits >> i & 1) sigaddset(&set, signals[i]);
    sigprocmask(SIG_SETMASK, &set, 0);
}

static int mask(void) {
    sigset_t set;
    int bits = 0;
    sigprocmask(SIG_BLOCK, 0, &set);
    for (int i = 0; i < 5; ++i) bits |= sigismember(&set, signals[i]) << i;
    return bits;
}

static sigjmp_buf env[BUFFERS + 1];
static int kind[VISITS], buffer[VISITS], copied[VISITS], visits, target[JUMPS], plain[JUMPS], jumps;
static volatile int save, visit, jumped, where, value;
static char cell[JUMPS * 4 + 1];

static _Noreturn void jump(int n) {
    set_mask(4 << n);
    if (plain[n]) longjmp(env[target[n]], n + 1);
    siglongjmp(env[target[n]], n + 1);
}

#define SITE(id, call)                                                                                       \
    switch (call) {                                                                                          \
    case 0: break;                                                                                           \
    case 1: where = id, value = 1; goto landed;                                                              \
    default: where = id, value = 2; goto landed;                                                             \
    }

static void scenario(void) {
    jumped = 0;
    for (visit = 0; visit < visits; ++visit) {
        set_mask(visit + 1);
        sigjmp_buf *b = &env[buffer[visit]];
        switch (kind[visit]) {
        case 0:
        case 1:
            save = kind[visit];
            SITE(0, sigsetjmp(*b, save))
            break;
        case 2: SITE(1, sigsetjmp(*b, 1)) break;
        case 3: SITE(2, sigsetjmp(*b, 0)) break;
        default: SITE(3, setjmp(*b)) break;
        }
        if (copied[visit]) memcpy(env[COPY], *b, sizeof env[COPY]);
    }
    jump(0);
landed:
    sprintf(cell + 3 * jumped, "%d%02x", value == jumped + 1 ? where : 9, mask());
    if (++jumped < jumps) jump(jumped);
}

static sigjmp_buf many[16], copies[16];
static volatile int at, step;

static void chain(void) {
    for (step = 0; step < 32; ++step) {
        at = step % 16;
        set_mask(step < 16 ? at : 15 - at);
        save = (at + (step >= 16)) & 1;
        if (sigsetjmp(many[at], save)) goto landed;
        if (step < 16) memcpy(copies[at], many[at], sizeof copies[at]);
    }
    at = -1;
landed:
    printf(" %d:%02x", at, mask());
    if (++at < 16) {
        set_mask(16);
        siglongjmp(at >> 1 & 1 ? copies[at] : many[at], 1);
    }
}

int main(void) {
    for (visits = 1; visits <= VISITS; ++visits) {
        int sequences = visits == 1 ? CHOICES : CHOICES * CHOICES;
        for (int s = 0; s < sequences; ++s) {
            int live[BUFFERS + 1] = {0};
            for (int v = 0, code = s; v < visits; ++v, code /= CHOICES) {
                kind[v] = code % KINDS;
                buffer[v] = code / KINDS % BUFFERS;
                copied[v] = code / (KINDS * BUFFERS) % 2;
                live[buffer[v]] = 1;
                live[COPY] |= copied[v];
                printf("%d%d%d ", kind[v], buffer[v], copied[v]);
            }
            printf("|");
            for (jumps = 1; jumps <= JUMPS; ++jumps) {
                int ways = jumps == 1 ? 6 : 36;
                for (int w = 0; w < ways; ++w) {
                    int ok = 1;
                    for (int j = 0, code = w; j < jumps; ++j, code /= 6) {
                        target[j] = code % 3;
                        plain[j] = code / 3 % 2;
                        ok &= live[target[j]];
                    }
                    if (!ok) {
                        printf(" -");
                        continue;
                    }
                    scenario();
                    printf(" %s", cell);
                }
            }
            printf("\n");
        }
    }
    set_mask(0);
    printf("chain");
    chain();
    printf("\n");
    return 0;
}
