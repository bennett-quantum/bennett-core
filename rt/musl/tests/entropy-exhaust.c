#include <sys/random.h>
int main(void) { char b[2]={9,9}; return getrandom(b,2,0)==2; }
