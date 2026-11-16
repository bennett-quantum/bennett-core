#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
mkdir -p .tmp/rust-probe/current
rustc --edition 2021 --crate-type lib -C panic=abort -C opt-level=2 --emit=llvm-bc,llvm-ir experiments/rust-probe/scalars.rs --out-dir .tmp/rust-probe/current
rustc -Vv > .tmp/rust-probe/current/version.txt
cargo build --manifest-path experiments/rust-probe/harness/Cargo.toml --target-dir target
out=.tmp/rust-probe/current
python3 - <<'PY'
from pathlib import Path
p=Path('.tmp/rust-probe/current/scalars.ll')
s=p.read_text()
assert s.count(' range(i8 37, 0)') == 2
assert s.count(' nocreateundeforpoison') == 2  # comment plus intrinsic declaration attrs
s=s.replace(' range(i8 37, 0)', '').replace(' nocreateundeforpoison', '').replace(', !"expected"', '')
p.with_name('attributes-dropped.ll').write_text(s)
PY
llvm-as-18 "$out/attributes-dropped.ll" -o "$out/attributes-dropped.bc"
target/front/bennett-front "$out/attributes-dropped.bc" -o "$out/attributes-dropped.bn" --region-budget 100000 --stack-bytes 4096 --emit-llvm "$out/lowered.ll"
rustc --edition 2021 -A unused-attributes -A unexpected_cfgs -C panic=abort -C opt-level=2 -C overflow-checks=off experiments/rust-probe/native.rs -o "$out/native"
"$out/native" > "$out/native.bin"
target/debug/rust-probe-harness "$out/attributes-dropped.bn" "$out/native.bin" all | tee "$out/results.tsv"
