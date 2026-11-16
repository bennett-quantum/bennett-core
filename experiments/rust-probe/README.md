# Rust scalar probe

Work in progress. This standalone experiment asks whether ordinary scalar Rust reaches
LLVM 18, `bennett-front`, and the executor, without changing the front.

`scalars.rs` has five exported C-ABI functions: two-argument wrapping arithmetic,
a one-argument branch, a one-argument data-dependent loop (0–255 iterations),
checked-add with a wrapping fallback, and ordinary `u8` addition, which panics on
overflow with checks enabled. `native.rs` includes the same source. The harness
compares native results on all 256 unary inputs and all 65,536 binary inputs,
then reverses every executed device run and compares the entire entry snapshot.

Rust 1.81.0 is chosen because its upstream release uses LLVM 18, the last Rust
release in that LLVM series, rather than trying to downgrade newer IR by editing it.
See Rust's [LLVM compatibility table](https://doc.rust-lang.org/rustc/linker-plugin-lto.html)
and the [LLVM 18 announcement](https://blog.rust-lang.org/2024/05/02/Rust-1.78.0/).
The compiler version is also checked locally. The repository itself and harness
use the current toolchain; only the probe sources use Rust 1.81.0.

From the repository root (LLVM/clang 18, CMake, Python 3 and rustup required):

```sh
rustup toolchain install 1.81.0 --profile minimal
CARGO_BUILD_JOBS=2 CMAKE_BUILD_PARALLEL_LEVEL=2 experiments/rust-probe/run.sh
```

On a shared machine, wrap the install and the full script with its heavy-job
limiter. The script writes build products, native outputs, input/lowered IR,
front diagnostics and result tables to ignored `.tmp/rust-probe/`. It runs four
configurations: Rust `-C opt-level=0/2` × `-C overflow-checks=off/on`, all with
`-C panic=abort`, `no_std`, one codegen unit and vectorisation disabled. Front
optimisation remains its normal O2 pipeline. No device or compiler changes.

## Measured compatibility stops

The initial host compiler reports `rustc 1.99.1` / `LLVM 23.1.1`. On the
wrapping (`opt-level=2`, overflow checks off) fixture, both routes fail:

```sh
rustc --edition 2021 --crate-type lib -C panic=abort -C opt-level=2 \
  --emit=llvm-bc,llvm-ir experiments/rust-probe/scalars.rs --out-dir .tmp/rust-probe/current
llvm-dis-18 .tmp/rust-probe/current/scalars.bc -o /dev/null
llvm-as-18 .tmp/rust-probe/current/scalars.ll -o /dev/null
```

The bitcode reader exits 1 with `Unknown attribute kind (105)` (producer
LLVM23.1.1, reader LLVM18.1.3). LLVM's [bitcode attribute enum](https://llvm.org/doxygen/LLVMBitCodes_8h_source.html)
names 105 `NO_CREATE_UNDEF_OR_POISON`; the textual IR has `nocreateundeforpoison`
on `llvm.fshl.i8`. The text assembler exits 1 with `expected value token` at
`i8 range(i8 37, 0) %a`, a call argument in `branchy`. Emitting text alone
does not bridge this version gap. Dropping those two attributes next reaches
another verifier failure: `Wrong number of operands` on
`!{!"branch_weights", !"expected", i32 1, i32 2000}`. LLVM 18 does not accept the
new `!"expected"` prediction operand. A newer reader or a validated IR downgrade
would be needed; the experiment uses a matching compiler instead.

## First device result: LLVM 23 textual IR with three compatibility edits

With the host compiler above, `current-text.sh` removes exactly the two call
argument `range` attributes, `nocreateundeforpoison` on the funnel-shift intrinsic,
and the `!"expected"` branch-prediction operand, then assembles with LLVM 18.
These drop optimizer promises/prediction information, not executable arithmetic.
This is a deliberately fixture-specific bridge, not a general LLVM downgrade.
It asserts the known attribute occurrence counts before changing the IR.

```sh
# Build bennett-front first (run.sh does this too).
cmake -S front -B target/front
cmake --build target/front --parallel 2
CARGO_BUILD_JOBS=2 experiments/rust-probe/current-text.sh
```

Measured on base `27265ba`: **197,120 native comparisons, 197,120 full-state
reversals, zero wrong results or device faults**. The compiler is opt-level 2,
overflow checks off; the front uses its normal O2 pipeline.

| Function | Inputs / returns / reversals | Steps min–max | Total steps | Peak journal bytes |
|---|---:|---:|---:|---:|
| arithmetic | 65,536 | 8–8 | 524,288 | 120 |
| branchy | 256 | 5–12 | 2,373 | 204 |
| counted_loop | 256 | 3–1,933 | 248,937 | 35,053 |
| checked | 65,536 | 6–6 | 393,216 | 102 |
| ordinary_add | 65,536 | 2–2 | 131,072 | 18 |

## Debug boundary, current compiler

At opt-level 0 with overflow checks on, unmodified LLVM 23 text also fails the
LLVM 18 parser at `trunc nuw` (`expected type`). `current-debug.sh` drops those
four promises and `nocreateundeforpoison`. Assembly then succeeds and the front
refuses the complete module with:

```text
not handled yet: a call to @_RNvNtNtCskADBYZwJmSU_4core9panicking11panic_const24panic_const_add_overflow, which has no definition (no library is linked yet), at in @ordinary_add
```

This is a `call` in the overflow arm of `llvm.uadd.with.overflow.i8`, not a
refusal of the checked-add intrinsic itself. It needs the appropriate `core`
bitcode plus a defined panic boundary. `panic=abort` prevents unwinding but does
not supply the missing Rust library definition in `--crate-type lib --emit`.
No input of this refused function was executed on the device in this route.
Native `ordinary_add(255, 1)` terminates with `SIGABRT` (core dumps disabled).

```sh
CARGO_BUILD_JOBS=2 experiments/rust-probe/current-debug.sh
```

Compiling the four non-panicking functions without `ordinary_add` then gives
**131,584 native comparisons and full-state reversals, zero wrong results or
faults**. Arithmetic and branch step counts match the release table; `checked`
takes 6 steps with a peak journal of 86 bytes. The loop takes 3–2,298 steps,
294,528 in total, peak journal 47,226 bytes. This route uses the same source and
normal front O2 pipeline; native compilation is opt-level 0, checks on.

The LLVM 18 producer and linked panic-boundary measurements are still pending.
