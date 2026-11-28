/* A program whose only main is weak and calls pull_entry, which an archive member defines (archive-entry.c) together
 * with the strong main that replaces this one, natively and in link.py, once the member is selected. */
int pull_entry(void);

__attribute__((weak)) int main(void) { return pull_entry(); }
