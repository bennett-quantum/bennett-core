# Rust scalar probe

**Yes, for these scalar functions.** Rust 1.99.1's LLVM 23 textual IR, with
explicit fixture-specific compatibility edits, assembles with LLVM 18 and runs
through the unchanged front and executor. **328,704 exhaustive native
comparisons and full entry-snapshot reversals pass**, with no wrong result or
unexpected device fault. Debug ordinary addition stops at a missing Rust core
panic definition, identified below; that refused function has no device runs.

`scalars.rs` has five exported C-ABI functions: two-argument wrapping arithmetic,
a one-argument branch, a one-argument data-dependent loop (0–255 iterations),
checked-add with a wrapping fallback, and ordinary `u8` addition, which panics on
overflow with checks enabled. `native.rs` includes the same source. The standalone
harness compares every return byte on all 256 unary inputs and all 65,536 binary
inputs, then reverses each accepted device run and compares the entire snapshot
(memory, frames, slots, stack, tape, outcome, steps, quantum state and worlds).
It runs outside the fast `cargo test` suite.

From the repository root (the measured compiler is Rust 1.99.1/LLVM 23.1.1;
LLVM/clang 18.1.3, CMake and Python 3 are required):

```sh
CARGO_BUILD_JOBS=2 CMAKE_BUILD_PARALLEL_LEVEL=2 experiments/rust-probe/run.sh
```

On a shared machine, wrap this command with its build/test limiter. It writes
build products, native bytes, input/lowered IR and result tables under ignored
`.tmp/rust-probe/`. The two configurations are opt-level 2/checks off and
opt-level 0/checks on, both with `no_std` and `panic=abort` on the bitcode input.
The native oracle uses std for output and the same scalar source and flags.
The front always uses its normal O2 pipeline. No front/executor changes, no
host toolchain defaults changed, and no Rust runtime was linked into the device module.

The chosen route is a measured, restricted text-IR conversion, not unmodified
LLVM 23 bitcode. A matching compiler is another route: Rust's
[LLVM compatibility table](https://doc.rust-lang.org/rustc/linker-plugin-lto.html)
lists 1.78–1.81 as LLVM 18 (see the
[LLVM 18 announcement](https://blog.rust-lang.org/2024/05/02/Rust-1.78.0/)).
For example, `rustup toolchain install 1.81.0 --profile minimal`, then explicit
`rustc +1.81.0`, avoids newer IR syntax without changing the default toolchain.
That producer route was not measured in this experiment; the text route below
is the one demonstrated end to end.

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
is needed; this experiment validates the restricted downgrade below.

## Device result: LLVM 23 textual IR with three compatibility edits

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

Measured on base `27265ba` (2026-11-16): **197,120 native comparisons, 197,120 full-state
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

## Scope and follow-up

The release configuration covers all five functions (197,120 inputs); the debug
configuration covers four (131,584). `ordinary_add` wraps on all release inputs;
with debug checks its input IR contains a panic arm for the 32,640 pairs whose
sum exceeds 255. Its whole module is refused before execution, including the
32,896 non-overflowing pairs. The native abort sample verifies the panic boundary;
it is not an exhaustive native panic sweep or a device/native panic comparison.

The missing definition needs matching `core` bitcode and a panic handler plus
its abort/effect boundary linked into the module, or a separately validated
lowering of Rust panic calls. `panic=abort` alone does not perform that link.
Those are necessary next steps, not claimed tested solutions. Unwinding, std,
allocation, Rust ABI aggregates, coherent calling and a general LLVM downgrade
are outside this scalar experiment. A matching LLVM 18 producer would remove
the format-compatibility edits; it would still need its panic dependencies.

The compatibility helpers deliberately touch only this fixture's known constructs.
The release helper checks exact occurrence counts. Both helpers assemble/verify
with LLVM 18, and the harness checks all accepted inputs; they are not a tool for
arbitrary newer Rust IR. Run the repository checks separately:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
front/test.sh
```


## Validation

2026-11-16, using the commands above: `cargo test` **292 passed, 19 ignored**;
`front/test.sh` **162 passed, 21 ignored**; `cargo clippy --all-targets -- -D warnings`
and `cargo fmt --check` pass. The standalone harness also passes Clippy with
warnings denied and its formatting check. A final `run.sh` replay reproduces
all nine result rows above: **328,704 matching native results, 328,704 exact
reversals, 2,514,291 forward steps**, zero wrong results or unexpected faults,
and the expected native debug abort sample and named front refusal.
