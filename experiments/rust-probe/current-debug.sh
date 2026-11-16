#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
mkdir -p .tmp/rust-probe/current-debug
cargo build --manifest-path experiments/rust-probe/harness/Cargo.toml --target-dir target
out=.tmp/rust-probe/current-debug
rustc --edition 2021 --crate-type lib -C panic=abort -C opt-level=0 -C overflow-checks=on --emit=llvm-ir experiments/rust-probe/scalars.rs -o "$out/scalars.ll"
sed -e 's/trunc nuw /trunc /g' -e 's/ nocreateundeforpoison//g' "$out/scalars.ll" > "$out/attributes-dropped.ll"
llvm-as-18 "$out/attributes-dropped.ll" -o "$out/attributes-dropped.bc"
if target/front/bennett-front "$out/attributes-dropped.bc" -o "$out/scalars.bn" --region-budget 100000 --stack-bytes 4096 > "$out/front.stdout" 2> "$out/front.stderr"; then
    echo 'expected the missing core panic definition refusal' >&2
    exit 1
fi
grep -q 'panic_const_add_overflow, which has no definition' "$out/front.stderr"
rustc --edition 2021 --cfg no_ordinary_add --crate-type lib -C panic=abort -C opt-level=0 -C overflow-checks=on --emit=llvm-ir experiments/rust-probe/scalars.rs -o "$out/safe.ll"
sed -e 's/trunc nuw /trunc /g' -e 's/ nocreateundeforpoison//g' "$out/safe.ll" > "$out/safe-dropped.ll"
llvm-as-18 "$out/safe-dropped.ll" -o "$out/safe.bc"
target/front/bennett-front "$out/safe.bc" -o "$out/safe.bn" --region-budget 100000 --stack-bytes 4096 --emit-llvm "$out/safe-lowered.ll"
rustc --edition 2021 -A unused-attributes -A unexpected_cfgs --cfg overflow_checks_probe -C panic=abort -C opt-level=0 -C overflow-checks=on experiments/rust-probe/native.rs -o "$out/native"
"$out/native" > "$out/native.bin"
ulimit -c 0
python3 - <<'PY'
import subprocess, signal
p=subprocess.run(['.tmp/rust-probe/current-debug/native','--panic-one','255','1'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
assert p.returncode == -signal.SIGABRT
assert not p.stdout
print('native ordinary_add(255,1): SIGABRT')
PY
target/debug/rust-probe-harness "$out/safe.bn" "$out/native.bin" safe-only | tee "$out/results.tsv"
