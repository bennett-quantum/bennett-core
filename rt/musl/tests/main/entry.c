/* The program's main, after helper.c (a private `main`) or fallback.c (a weak one): reads its arguments. Run with
 * the one argument `x`: returns 3 through helper.c's function, or 2 (argc) after fallback.c. */
int from_helper(void);

int main(int argc, char **argv) {
#ifdef WITH_HELPER
    return argc == 2 && argv[1][0] == 'x' ? from_helper() : 40;
#else
    return argc;
#endif
}
