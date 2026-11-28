/* A program that defines the scope list itself, linked before scope-push.c, which carries the header's
 * zero-initialised weak definition. The header declared the list weak, so this definition is weak too, and the
 * linker keeps the first of the weak definitions: this one, since this unit comes first, in a native link and in
 * link.py alike. Returns 0 if the list starts as defined here and a push in the other unit lands on it. */
#include <bq.h>
#include <stdio.h>

void enter(uint64_t id, uint64_t bit);

static bq_bit entries[4] = { { { 5 }, 2 } };
struct bq_scope bq_scope_ = { entries, 1, 4 };

int main(void) {
    enter(6, 0);
    printf("depth %llu handle %llu\n", (unsigned long long)bq_scope_depth(),
           (unsigned long long)bq_scope_list()[0].reg.id);
    return bq_scope_depth() == 2 && bq_scope_list()[0].reg.id == 5 && bq_scope_list()[1].reg.id == 6 ? 0 : 1;
}
