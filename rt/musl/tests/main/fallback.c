/* A weak main without parameters, in the program's first unit; entry.c's external main replaces it. */
__attribute__((weak)) int main(void) { return 80; }
