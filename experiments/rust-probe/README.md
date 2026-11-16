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
does not bridge this version gap. A newer reader or a validated IR downgrade
would be needed; the experiment uses a matching compiler instead.

Device measurements and remaining blockers will be recorded after execution.
