# Automatic checks

Run `scripts/check.sh` for the four repository checks and the tree guards. Rust tests also run the
small guard self-tests (Python 3 required); LLVM is needed only for `front/test.sh`. All checks use
source discovery, compiler exhaustiveness, or exact outcomes: no coverage percentage, timing limit,
allowed-hit baseline, or list of example names is maintained.

| Check | Entry point and rejection witness |
|---|---|
| Changed-line mutation | `python3 scripts/mutate-diff.py origin/main`, **after committing the step and before landing**. A surviving replacement makes cargo-mutants fail. `--list` previews scope and is not a passing mutation run. |
| Toolchain pin | `front/test.sh`, CMake configuration, and `front/tests/tests/toolchain.rs`. The actual clang, linked front LLVM, and runtime bitcode linker must have the major in `front/llvm-major.txt` (18). A wrong compiler on PATH or through CLANG is rejected. Patch releases are deliberately compatible with the existing LLVM 18 pin. |
| Validator admission | `cargo test -p bennett-ir --test admission`: accepted boundary programs round-trip; rejected programs require the exact validation problem, both reachable and in unused functions. Reader error cases remain in `text.rs`. |
| Example discovery | `cargo test -p bennett-ir --test text`: recursively discovers `examples/**/*.bn`; an added malformed example fails without changing a list. Executor tests separately check the examples' outcomes. |
| Attribute classification | `front/test.sh` compiles and runs `front/tests/checks.cpp`. Its exhaustive switch includes `front/src/Attributes.def`; deleting a policy, or adding an LLVM enum kind, fails compilation. Reasons distinguish explicit lowering, LLVM promises/hints with no device encoding, and named refusals. |
| Exact pushed revision | `.githooks/pre-push` archives each nondeleted SHA from Git's pre-push input, then runs `scripts/check.sh` in that isolated tree. A bad non-HEAD branch fails even when HEAD passes; dirty files and existing front binaries cannot repair it. |
| Tree hygiene | `python3 scripts/checks.py`: tracked and nonignored untracked source is scanned for host-specific home paths, nonlocal symlinks and generated/session litter. The archive hook scans the exact committed manifest. |
| No WIP on main | The same hook checks **all incoming commits**, including ancestors and a newly created main. WIP/work-in-progress/fixup/squash subjects fail on `refs/heads/main`; topic and `wip/` refs may contain them. |
| Exhaustive library matches | The IR and executor libraries deny `clippy::wildcard_enum_match_arm` outside their unit tests. `cargo clippy --all-targets -- -D warnings` enforces this; `scripts/check.sh` also runs clippy over **all workspace libraries** with this lint, discovering new library targets automatically. Test assertions may use catch-all arms. |
| Bare script durations | `python3 scripts/checks.py`: Python sleep/timeout literals and shell timeout/sleep literals are flagged; JavaScript timer literals are also flagged. Scripts are selected by extension (`.py`, `.sh`, `.js`, `.ts`) and, for every other file, by a shebang line (`python*`; `sh`, `bash`, `dash`, `ksh`, `zsh`, `ash`; `node`, `deno`, `bun`, `ts-node`; directly or through `env` and its options), so extensionless hooks such as `.githooks/pre-push` are read. A shebang naming any other interpreter is reported as a problem, never skipped. Name durations with their unit and purpose, e.g. `NATIVE_TIMEOUT_SECONDS`. |

The hook witnesses and tree-guard witnesses are executable in `python3 scripts/test_checks.py`, also
called by `cargo test`. They create disposable repositories under `.tmp`, never push anything, and
exercise bad non-HEAD refs, a dirty checkout, branch deletion, and a WIP ancestor below a finished tip.

## Install the push hook

From this checkout:

```sh
git config --local core.hooksPath .githooks
```

This selects the versioned hook for this repository (including its worktrees). It replaces any existing
hooksPath setting, so combine hooks if another policy is already installed. The hook does not install
itself. A remote old SHA that is not available locally fails closed; fetch that remote before retrying.
It checks every nondeleted pushed revision, including checkpoint branches. No log from an earlier run
is accepted as evidence. Git's `--no-verify` can bypass local hooks; server-side enforcement is separate.

On a shared host, export `CARGO_BUILD_JOBS=2` and set `BENNETT_BUILD_WRAPPER` to a local executable that
accepts `build <command ...>`. `scripts/check.sh` routes builds/tests through it. The hook preserves that
setting. The default `front/test.sh` also caps CMake jobs using `CARGO_BUILD_JOBS` (default 2).

## Mutation testing is a separate landing step

Install the established tool with `cargo install cargo-mutants --locked`. Commit a step, then run:

```sh
python3 scripts/mutate-diff.py origin/main --list
python3 scripts/mutate-diff.py origin/main
```

The script checks a clean revision, uses its merge-base diff, rejects an empty mutation set, runs one mutant at a time, and generates mutations across the workspace. It tests the workspace default packages plus every
changed Rust package, discovered through Cargo metadata, rather than just the mutated crate. For
front-harness changes, run `front/test.sh` first; the runner supplies the built front to isolated test
copies through `BENNETT_FRONT`. Results and the input diff go under
`.tmp/mutation/`. Fix every real surviving mutant with a regression test; examine unviable mutants and
timeouts rather than relabeling them as caught. Do not lower a score threshold or exclude a survivor to
land. For unsupported forms, seed a semantic edit by hand and run the relevant existing suite.

Scope and cost are explicit: cargo-mutants generates Rust mutations, not C++, Python, or shell
mutations. Front/script edits therefore require recorded seeded edits and their actual failing check;
the script's zero exit alone does not certify them. A front harness change also needs its LLVM-dependent
suite. Automatic changed-line mutation for those languages remains a debt (see HANDOFF). Enumeration
and an individual small witness take seconds; a full changed-line run scales with the number of
mutants and rebuild cost. No honest full mutation run has a size-independent seconds guarantee. Keep
it out of `cargo test`; record its revision, command, caught/missed/unviable/timeout counts and elapsed
work before landing. Cold builds and the isolated pre-push front build likewise take longer than warm
checks. These are resource costs, not semantic failures or wall-time gates.

The attribute policy covers every LLVM **enum** kind, discovered by the C++ compiler. LLVM's open-ended
string attributes cannot have a closed exhaustive list. Most enum promises are consumed by LLVM's
optimiser; classification is not a proof of support for every placement or custom calling convention.
The reasons record conditional handling and current debts. Native differential tests remain necessary.


## Measured attribute placements and native comparisons (2026-11-17)

The inventory is a measurement of clang **18.1.3**, target `x86_64-pc-linux-gnu`, before any
front pass. It counts attributes on **definitions, declarations and call sites**, separately at
**function, return and parameter** placement. Each parameter occurrence counts once; the same
attribute on a declaration and on its call counts twice. LLVM integer- and type-valued kinds
(`align`, `memory`, `byval`, `sret`, etc.) are included with the enum kinds. Instruction flags,
metadata, load/store alignment and calling-convention IDs are different IR properties and are
not counted as AttributeSets. This is an observed input envelope, not an allowlist or a replacement
for the compiler-exhaustive enum switch.

`front/tests/attribute-inventory.cpp` reads LLVM's AttributeSets rather than parsing printed function
signatures. `front/tests/attribute-inventory.py` compiles each source/configuration at both `-O0`
(with O0 optnone disabled) and `-O2`, with vectorisers disabled as the front requires. The three
corpora use the existing harness's dialect options and installed musl headers; `ieee/` also uses
`-fno-inline`. Musl uses **all 1,345 C members selected by its own build**, the exact include/overlay
flags in `target/musl/cflags.json`, and both optimisation levels; its five runtime/startup/glue C
files are counted separately. Lua is the verified 5.4.9 release (checksum in its probe), all 32
library files plus its driver and oracle embeddings, with the probe's musl headers and defines.
The 1,656 GCC inputs are GCC 14.2.0 (`04696df09633`)'s top-level torture execute tests; `ieee/` has 68 and
c-testsuite (`5c7275656d75`) has 220. Compiler failures are recorded rather than silently omitted.

Front coverage includes every checked-in C/C++ fixture, including ignored-test fixtures, plus
actual successful/default-harness compile configurations and generated C, recorded by
`attribute-record-clang.py`. The wrapper snapshots each source version before clang runs, so
reusing a generated filename does not erase earlier versions. Historical snapshots of edited checked-in
fixtures are excluded when their bytes differ from the current checkout. Harness replay retains defines,
target flags and frontend options, replaces the optimisation level, and adds the source's original
quoted-include directory. Static fixtures and recorded configurations intentionally overlap:
counts describe compile inputs, not unique source functions. Explicit denormal defect witnesses
and the musl allocation comparison are also recorded. Hand-authored `.ll` fixtures are not
clang C/C++ output and are excluded from this emission inventory. Generated sources that only
ignored experiments construct are outside the measured default-harness envelope; their checked-in
C/C++ fixtures are included.

The published [complete counts](attribute-inventory.tsv) retain dataset, optimisation level, site,
placement, enum/string category, exact attribute spelling/value and occurrence count. The summary
below sums over sites and datasets, retaining placement and optimisation level. No count is a gate.
Local `sources.json` records source hashes and every failed compilation/LLVM read; `summary.json`
records the compiler and build counts. The intentional over-alignment fixture makes LLVM's bitcode debug
upgrader abort. For measurement only, the script recompiles the same source/options to textual LLVM with
debug metadata disabled (`-g0`) and reads its AttributeSets without front admission; the invalid alignment
is counted explicitly and remains a named input refusal. Any unrecoverable reader failure is recorded separately.


### Inventory counts

Observed **43 enum kinds and 16 string keys**. All observed string attributes use function placement; string return/parameter counts are zero. Each cell below sums definitions, declarations and calls. Exact string values and enum payloads remain in the TSV.

| Input dataset | O0 emitted / failed | O2 emitted / failed |
|---|---:|---:|
| cts | 220 / 0 | 220 / 0 |
| front-harness | 421 / 0 | 420 / 1 |
| front-static | 99 / 1 | 99 / 1 |
| gcc | 1607 / 49 | 1607 / 49 |
| ieee | 68 / 0 | 68 / 0 |
| lua | 34 / 0 | 34 / 0 |
| musl | 1345 / 0 | 1345 / 0 |
| musl-glue | 5 / 0 | 5 / 0 |

Total: **7597 emitted modules** and **101 unsuccessful compilations**. The 8 invalid over-alignment fixture/configuration builds counted via LLVM text recovery are included as emitted modules; they remain rejected by the front. No unrecovered LLVM-reader failures.

| Category | Attribute key | Placement | O0 occurrences | O2 occurrences |
|---|---|---|---:|---:|
| enum | `align` | parameter | 11227 | 4698 |
| enum | `align` | return | 22 | 342 |
| enum | `allocalign` | parameter | 0 | 2 |
| enum | `allockind` | function | 0 | 37 |
| enum | `allocptr` | parameter | 0 | 9 |
| enum | `allocsize` | function | 59 | 67 |
| enum | `byval` | parameter | 3297 | 2504 |
| enum | `cold` | function | 21 | 38 |
| enum | `convergent` | function | 6 | 0 |
| enum | `dead_on_unwind` | parameter | 348 | 291 |
| enum | `dereferenceable` | parameter | 67 | 4900 |
| enum | `dereferenceable` | return | 20 | 20 |
| enum | `dereferenceable_or_null` | return | 0 | 37 |
| enum | `elementtype` | parameter | 32 | 31 |
| enum | `hot` | function | 2 | 2 |
| enum | `immarg` | parameter | 867 | 2345 |
| enum | `inlinehint` | function | 0 | 30 |
| enum | `memory` | function | 2311 | 9988 |
| enum | `mustprogress` | function | 20 | 5466 |
| enum | `noalias` | parameter | 1588 | 941 |
| enum | `noalias` | return | 13 | 84 |
| enum | `nobuiltin` | function | 7256 | 5583 |
| enum | `nocallback` | function | 2022 | 3795 |
| enum | `nocapture` | parameter | 1242 | 5293 |
| enum | `nofpclass` | parameter | 6 | 6 |
| enum | `nofpclass` | return | 6 | 6 |
| enum | `nofree` | function | 2022 | 10779 |
| enum | `noinline` | function | 13965 | 2546 |
| enum | `nonnull` | parameter | 76 | 29884 |
| enum | `nonnull` | return | 20 | 98 |
| enum | `norecurse` | function | 0 | 6092 |
| enum | `noreturn` | function | 9104 | 6972 |
| enum | `nosync` | function | 1296 | 9300 |
| enum | `noundef` | parameter | 101682 | 86474 |
| enum | `noundef` | return | 45 | 4501 |
| enum | `nounwind` | function | 19957 | 42328 |
| enum | `readnone` | parameter | 0 | 449 |
| enum | `readonly` | parameter | 516 | 1576 |
| enum | `returned` | parameter | 0 | 136 |
| enum | `returns_twice` | function | 114 | 114 |
| enum | `signext` | parameter | 732 | 720 |
| enum | `signext` | return | 230 | 108 |
| enum | `speculatable` | function | 815 | 1131 |
| enum | `sret` | parameter | 348 | 291 |
| enum | `strictfp` | function | 275 | 267 |
| enum | `uwtable` | function | 10963 | 9609 |
| enum | `willreturn` | function | 2613 | 9617 |
| enum | `writable` | parameter | 348 | 291 |
| enum | `writeonly` | parameter | 726 | 901 |
| enum | `zeroext` | parameter | 805 | 447 |
| enum | `zeroext` | return | 668 | 397 |
| string | `alloc-family` | function | 0 | 37 |
| string | `denormal-fp-math` | function | 4 | 4 |
| string | `denormal-fp-math-f32` | function | 4 | 4 |
| string | `disable-tail-calls` | function | 38 | 38 |
| string | `frame-pointer` | function | 21566 | 38 |
| string | `min-legal-vector-width` | function | 13965 | 11492 |
| string | `no-builtin-abort` | function | 43 | 37 |
| string | `no-builtins` | function | 13254 | 10384 |
| string | `no-infs-fp-math` | function | 6 | 6 |
| string | `no-nans-fp-math` | function | 6 | 6 |
| string | `no-signed-zeros-fp-math` | function | 4 | 4 |
| string | `no-trapping-math` | function | 21566 | 18313 |
| string | `stack-protector-buffer-size` | function | 21566 | 18313 |
| string | `target-cpu` | function | 21566 | 18313 |
| string | `target-features` | function | 21566 | 18313 |
| string | `tune-cpu` | function | 21566 | 18313 |

The 49 GCC source failures are the same at both levels (unsupported GCC extensions or source/dialect/header diagnostics); they have no emitted module. The static SoftFloat fixture requires its harness defines/includes, whose configured builds are included separately. The O2 replay of the intentional optimiser-crash C fixture also fails in clang before a module is written. These are counted failures, not missing attribute rows.

### Semantic review and executed witnesses

Policies below were checked against `front/src/Attributes.def`, `Abi.cpp`, `Normalise.cpp` and
`SoftFloat.cpp`; there is no general string-attribute classifier in production. LLVM sees the
attributes during optimisation. Its open-ended string set is not made exhaustive by this sample.
The new tests are in `front/tests/tests/attributes_independent.rs`.

| Attribute(s), observed placement | Current policy and result-sensitive comparison |
|---|---|
| `signext`, `zeroext`, return and parameter | `Abi.cpp` explicitly extends/checks adapted narrow arguments; correctly typed device slots already retain the declared width. Signed char, unsigned char and bool returns/parameters through volatile function pointers compare boundary and high-bit inputs. 112 exact native/device words, both levels. The existing `proto_independent` matrix adds mismatched-prototype paths; native upper return-register bits are deliberately not promised. |
| `byval`, `sret`, parameter; `align`, parameter and return | `Normalise.cpp` makes callee-owned by-value copies, preserves alignment and clears tail promises; `Abi.cpp` handles the hidden result pointer. The new C fixture mutates a three-word argument, reads the caller's original afterwards, returns another three-word object and passes a 64-byte-aligned object. Included in the 112 words above. Existing `front_call_independent` adds tail calls, recursive copies and every valid power-of-two byval alignment through 16,384. |
| `returned`, parameter; `nonnull`, `noundef`, `dereferenceable`, parameter/return; `readnone`, `readonly`, `nocapture`, parameter | LLVM optimisation facts, no device operation on valid inputs. C++ references and member calls mutate and reread aliased objects and return references; 48 exact words across both levels. The fixture guard asserts actual return/parameter placement at both levels and the O2 `returned` fact. No comparison intentionally violates a promise: that would be LLVM undefined behavior, not an oracle. |
| `noalias`, return/parameter; `allocsize`, `allockind`, function; `allocalign`, `allocptr`, parameter; `dereferenceable_or_null`, return; `"alloc-family"` | Optimisation/allocation facts consumed by LLVM; no separate device allocation semantics. The musl-linked allocation fixture checks 64-byte alignment, disjoint live objects, memcpy, calloc zeros, realloc preservation, free/reallocation, and 16 independent hashes per level against executed native stdout/status. Both program optimisation inputs use musl as built by its existing O0 recipe. |
| `memory`, function; `writeonly`, `writable`, `dead_on_unwind`, parameter; `nofree`, `nosync`, function | LLVM memory/effect promises; byval lowering repairs effects after inserting a read/copy. The byval, reference and allocation witnesses retain live values across writes/copies/calls; an incorrect effect could fold these to a different hash. The exception-only part of `dead_on_unwind` has no C unwinding witness here. |
| `nofpclass`, return/parameter | `SoftFloat.cpp` drops class promises after floating types become integer bit containers. A noninlined floating argument/return under no-NaN/no-infinity options is compared bitwise with native. The guard requires `nofpclass(nan inf)` on both placements. |
| `strictfp`, function | The table records a refusal through constrained floating intrinsics, not a blanket ban on every function bearing the attribute. Native strict code runs at both levels; the front refuses its constrained arithmetic by name at both levels. Musl can emit strictfp on an asm/environment helper without constrained arithmetic; counting the attribute alone is not a claim that strict arithmetic is supported. The floating environment remains the recorded fallback debt. |
| `"denormal-fp-math"`, `"denormal-fp-math-f32"`, function | **Wrong result, accepted silently.** No lowering consults these strings; SoftFloat retains IEEE subnormals. With native MXCSR DAZ/FTZ explicitly set to satisfy `preserve-sign,preserve-sign`, the general witness differs on 48/96 words, the f32-only witness on 12/32, counting both levels. Each is a separate ignored defect test, explicitly run and observed failing. The smallest positive subnormal doubled is native zero/device bit pattern 2. Output flushing of the smallest normal times one half and signed zeros are also covered. Route: honor input/output modes at the SoftFloat boundary or refuse non-IEEE modes by name until that lowering exists; do not silently discard them. |
| `"no-trapping-math"`, `"no-infs-fp-math"`, `"no-nans-fp-math"`, `"no-signed-zeros-fp-math"` | LLVM permissions/facts, no device trap or environment encoding. IEEE/default, no-trapping and finite/no-signed-zero/reciprocal settings give 288 exact native/device words across both levels, including subnormals, signed inputs, cancellation and a noninlined FP return. Inputs satisfy the finite-value promises. This does not promise enabled exceptions or arbitrary ambient rounding modes. |
| `nobuiltin`, `"no-builtins"`, `"no-builtin-abort"`, function | LLVM consumes builtin suppression before device lowering; defined program functions must keep their bodies. A deliberately nonstandard user `abs` under general suppression and a returning user `abort` under only `-fno-builtin-abort` give 256 exact words, both levels. The second does not carry `no-builtins`, so the individual suppression is exercised independently. Existing floating tests likewise preserve a program's own runtime-named function. |
| `"disable-tail-calls"`, `"frame-pointer"`, `"stack-protector-buffer-size"`, `"target-cpu"`, `"target-features"`, `"tune-cpu"`, `"min-legal-vector-width"`, function | Host code-generation options; no device encoding. The native comparisons exercise the default x86-64 target. Existing byval/vararg tests also compare scalar data under baseline/AVX2/AVX512 lowering, with true native AVX2. This CPU has no AVX512F: native AVX512 execution and every target-feature combination are not claimed. The independent return-address suite checks nested/recursive frame observations; `disable-tail-calls` is consumed by LLVM before ordinary device calls. These strings can affect clang's earlier ABI/instruction choice, which the front must still lower or refuse individually. |
| `noinline`, `inlinehint`, `hot`, `cold`, `uwtable`, function | Optimisation/code-generation hints; optnone and its associated noinline are removed by `Normalise.cpp`; the inventory disables O0 optnone at compilation. Native comparisons use runtime inputs and externally visible results, not inlining or frame layout as an oracle. Target frame/return-address values outside the documented device convention remain the existing debt. |
| `mustprogress`, `willreturn`, `norecurse`, `noreturn`, `nounwind`, `returns_twice`, function | Valid execution/control-flow facts. No-return control flow and non-local jumps have explicit front lowering; the existing default setjmp, retaddr and independent prototype suites ran in the recorded front check. Infinite loops violating mustprogress, returning from noreturn and jumps to returned C frames are not valid semantic counterexamples. |
| `convergent`, `nocallback`, `speculatable`, function; `immarg`, `elementtype`, parameter | Primarily intrinsic/inline-asm contracts in these inputs. LLVM intrinsic expansion and the existing exact FP/memory tests check executed operations; nonempty inline assembly is still refused individually. This inventory is not evidence of arbitrary convergent GPU operations or custom inline-asm conventions. |

`inreg`, `preallocated` and `inalloca` are absent from the measured clang-generated default x86-64
C/C++ inputs. They have **zero measured occurrences**, not passing native comparisons. The table's
conditional `inreg` prototype refusal and named `preallocated`/`inalloca` parameter refusals remain
unchanged. New target conventions and LLVM IR authored outside this clang envelope need their own
native witnesses. The same applies to other enum kinds absent from the counts.

Two witnesses fail by design and stay ignored with `defect:` reasons; the separately built musl
prerequisite test is ignored by default and was executed explicitly. No production code changed.
The placement guard checks the exact attributes the new ABI/reference/FP comparisons claim to
exercise. The inventory reader's independent self-test checks complete literal maps at every
site/placement and duplicate-module multiplicity.

Reproduce (corpus source locations and verified Lua archive are inputs, not downloads by this script):

```sh
mkdir -p .tmp
python3 rt/musl/build.py --jobs 3
export ATTRIBUTE_CLANG=clang-18
export ATTRIBUTE_CLANG_LOG="$PWD/.tmp/attribute-commands.jsonl"
export CLANG="$PWD/front/tests/attribute-record-clang.py"
front/test.sh
# Includes two expected failing defect tests; also executes the musl prerequisite test.
cargo test -p bennett-front-tests --test attributes_independent -- --include-ignored --nocapture
export CLANG=clang-18
python3 front/tests/attribute-inventory.py --gcc "$GCC_EXECUTE" --cts "$CTS_SINGLE_EXEC" \
  --lua-archive "$LUA_ARCHIVE" --front-commands .tmp/attribute-commands.jsonl --jobs 3
python3 front/tests/test_attribute_inventory.py .tmp/attribute-inventory/read-attributes
```

Use the shared build/heavy wrapper and `CARGO_BUILD_JOBS=3` on the shared machine. The measurement
ran separately for `--groups gcc,ieee,cts,musl,musl-glue,lua` and
`--groups front-static,front-harness --out .tmp/attribute-front-final`; their disjoint dataset rows were
combined in the published TSV. Clear the recorder JSONL and its source directory for a fresh default
harness measurement; otherwise it intentionally accumulates additional configurations. The input
pin permits LLVM 18 patch releases; these exact counts describe 18.1.3, not every permitted patch.


Inventory-tool validation: the exact self-test passes on the unchanged reader. A scratch copy
with `record(list.getRetAttrs(), "return")` removed was compiled separately and the self-test was
seen failing (one viable seed, one caught); production and the original reader were untouched.
Disabling debug metadata left every counted AttributeSet identical on the valid ABI control at both
optimisation levels. The recorder self-test overwrites one generated filename twice and requires both byte-for-byte
source snapshots. These are explicit seeded/tool checks, not a full C++/Python mutation claim.


Final required checks after rebasing on `d3880f6`: `cargo test` **499 passed / 29 ignored**;
`front/test.sh` **210 passed / 36 ignored**, including the C++ header and all 88 enum classifications;
`cargo clippy --all-targets -- -D warnings`, the additional workspace-library lint and `cargo fmt --check`
passed. The new test target separately passes `cargo clippy -p bennett-front-tests --test attributes_independent -- -D warnings`.
The explicit new-suite run is **6 passed / 2 failed**, with exactly the two denormal defects described above:
**704 exact result words and 32 allocation hashes agree**, strict native runs produce 48 words per level
before the front's named refusal, and both defect tests evaluate both levels before reporting mismatches.
An optional broader `cargo clippy -p bennett-front-tests --all-targets -- -D warnings` fails on two pre-existing
`unwrap()` uses in `front/tests/tests/complex.rs` (the archive/commutation experiment paths); no attribute-test
lint fails, and this pass does not change those unrelated tests.
