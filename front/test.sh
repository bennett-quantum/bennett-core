#!/bin/sh
# Builds bennett-front and runs its tests: C through clang and bennett-front, run by the executor.
# Needs CMake, a C++17 compiler, LLVM 18 (found by CMake, or set LLVM_DIR to its CMake directory) and
# clang 18 (on the path, or set CLANG). Extra arguments go to `cargo test`.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
build="$root/target/front"
if [ -n "${LLVM_DIR:-}" ]; then
  cmake -S "$root/front" -B "$build" -DLLVM_DIR="$LLVM_DIR" >/dev/null
else
  cmake -S "$root/front" -B "$build" >/dev/null
fi
cmake --build "$build" --parallel >/dev/null
cargo test --manifest-path "$root/Cargo.toml" -p bennett-front-tests "$@"
