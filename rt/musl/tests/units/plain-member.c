/* A function test.py compiles with plain clang into an archive member of a copied library directory, to check that
 * link.py refuses a member without bennett-cc's mark. */
int plain_member(void) { return 7; }
