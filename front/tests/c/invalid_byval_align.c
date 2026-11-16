/* An argument passed by value and aligned past LLVM's limit of 2^14 bytes: clang 18 writes a call whose
   `byval` carries `align 1048576`, which LLVM's verifier rejects. The input is not valid LLVM IR, and the front
   says so by name (`front/tests/tests/invalid_input.rs`). */
struct huge { char c; } __attribute__((aligned(1048576)));

int take(struct huge h) { return h.c; }

int main(void) {
  struct huge h = {7};
  return take(h);
}
