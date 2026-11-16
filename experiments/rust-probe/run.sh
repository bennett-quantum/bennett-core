#!/usr/bin/env bash
# Standalone experiment; run under the caller's build/test resource limiter.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export CMAKE_BUILD_PARALLEL_LEVEL=${CMAKE_BUILD_PARALLEL_LEVEL:-2}
unset DISPLAY WAYLAND_DISPLAY
mkdir -p .tmp/rust-probe
cmake -S front -B target/front > .tmp/rust-probe/cmake.log
cmake --build target/front --parallel "$CMAKE_BUILD_PARALLEL_LEVEL" >> .tmp/rust-probe/cmake.log 2>&1
experiments/rust-probe/current-text.sh
experiments/rust-probe/current-debug.sh
