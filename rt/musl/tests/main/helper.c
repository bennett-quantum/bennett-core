/* A private function whose symbol name is main, in the program's first unit: not the program's entry. Native start-up
 * calls the external main of entry.c; link.py must give start-up that one's parameters too. */
static int helper(void) __asm__("main");
static int helper(void) { return 3; }

int from_helper(void) { return helper(); }
