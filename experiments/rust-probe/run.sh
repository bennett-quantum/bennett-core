#!/usr/bin/env bash
# Run under the caller's resource wrapper; this is intentionally outside cargo test.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export CMAKE_BUILD_PARALLEL_LEVEL=${CMAKE_BUILD_PARALLEL_LEVEL:-2}
ulimit -c 0
unset DISPLAY WAYLAND_DISPLAY
out="$root/.tmp/rust-probe"
mkdir -p "$out"
probe="$root/experiments/rust-probe"
toolchain=${RUST_PROBE_TOOLCHAIN:-1.81.0}
rustc +"$toolchain" -Vv > "$out/rustc.txt"
grep -q 'LLVM version: 18\.' "$out/rustc.txt"
llvm-as-18 --version > "$out/llvm.txt"
cmake -S front -B target/front > "$out/cmake.log"
cmake --build target/front --parallel "$CMAKE_BUILD_PARALLEL_LEVEL" >> "$out/cmake.log" 2>&1
cargo build --manifest-path "$probe/harness/Cargo.toml" --target-dir "$root/target" > "$out/harness-build.log" 2>&1
front="$root/target/front/bennett-front"

for opt in 0 2; do
    for checks in off on; do
        tag="o${opt}-${checks}"
        cfg=()
        if [[ $checks == on ]]; then cfg=(--cfg overflow_checks_probe); fi
        flags=(--edition 2021 -A unused-attributes -C "opt-level=$opt" -C "overflow-checks=$checks" -C panic=abort -C codegen-units=1 -C no-vectorize-loops -C no-vectorize-slp)
        rustc +"$toolchain" "${flags[@]}" --crate-type lib --emit=llvm-bc,llvm-ir "$probe/scalars.rs" --out-dir "$out/$tag"
        llvm-dis-18 "$out/$tag/scalars.bc" -o "$out/$tag/read-by-18.ll"
        llvm-as-18 "$out/$tag/scalars.ll" -o "$out/$tag/text-roundtrip.bc"
        rustc +"$toolchain" "${flags[@]}" "${cfg[@]}" "$probe/native.rs" -o "$out/$tag/native"
        "$out/$tag/native" > "$out/$tag/native.bin"
        if [[ $checks == on ]]; then
            python3 "$probe/check-panics.py" "$out/$tag/native" | tee "$out/$tag/native-panics.tsv"
        fi
        if "$front" "$out/$tag/scalars.bc" -o "$out/$tag/scalars.bn" --region-budget 100000 --stack-bytes 4096 --emit-llvm "$out/$tag/lowered.ll" > "$out/$tag/front.stdout" 2> "$out/$tag/front.stderr"; then
            mode=all
        else
            # Keep the refusal, then measure the four functions that never panic.
            [[ $checks == on ]]
            rustc +"$toolchain" "${flags[@]}" --cfg no_ordinary_add --crate-type lib --emit=llvm-bc "$probe/scalars.rs" -o "$out/$tag/safe.bc"
            "$front" "$out/$tag/safe.bc" -o "$out/$tag/scalars.bn" --region-budget 100000 --stack-bytes 4096 --emit-llvm "$out/$tag/safe-lowered.ll" > "$out/$tag/safe-front.stdout" 2> "$out/$tag/safe-front.stderr"
            mode=safe-only
        fi
        "$root/target/debug/rust-probe-harness" "$out/$tag/scalars.bn" "$out/$tag/native.bin" "$mode" | tee "$out/$tag/results.tsv"
    done
done
