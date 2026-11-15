/* The same function with internal linkage: LLVM's pipeline inside the front deletes it. */
static double half(double x) { return x / 2; }

int main(void) { return 0; }
