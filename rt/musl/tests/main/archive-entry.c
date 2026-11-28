/* An archive member that test.py adds to a copied library directory: pull_entry selects it, and its main replaces the
 * program's weak one. Run with the one argument `x`: returns 3. */
int pull_entry(void) { return 80; }

int main(int argc, char **argv) { return argc == 2 && argv[1][0] == 'x' ? 3 : 40; }
