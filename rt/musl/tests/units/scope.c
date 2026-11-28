/* A unit that includes bq.h and reads its scope list without calling a primitive: returns 7. Linked as
 * preprocessed C (.i) by test.py, preprocessed with musl's headers. */
#include <bq.h>

int main(void) { return 7 + (int)bq_scope_depth(); }
