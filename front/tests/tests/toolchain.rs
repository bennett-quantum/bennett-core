//! Pin check is a standalone test as well as a precondition of every compilation.
#[test]
fn clang_and_front_use_the_pinned_llvm_major() {
    let _ = bennett_front_tests::clang();
}
