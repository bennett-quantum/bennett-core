#!/bin/sh
# Four repository checks plus the fast tree guards. A shared host may supply a build wrapper.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
python3 scripts/checks.py
run_build() {
  if [ -n "${BENNETT_BUILD_WRAPPER:-}" ]; then
    "$BENNETT_BUILD_WRAPPER" build "$@"
  else
    "$@"
  fi
}
run_build cargo test --locked
run_build cargo clippy --locked --all-targets -- -D warnings
run_build cargo clippy --workspace --lib -- -D warnings -D clippy::wildcard_enum_match_arm
cargo fmt --check
run_build front/test.sh
