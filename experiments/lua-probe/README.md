# Lua on the device

Stage 1 passes: **unmodified PUC-Rio Lua 5.4.9**, all 32 library translation
units compiled with clang 18, linked through musl and the unchanged front.
Seven embedded scripts agree with native on stdout bytes and exit status.
Stage 2 also passes: every script restores its complete entry snapshot exactly.
Stage 3 also passes: a Lua predicate agrees with native in superposition, and
Grover at N=4/8 agrees with an independent dense calculation. The C marker
front still refuses the memory-writing predicate call.

The latest 5.4 release on the [official download page](https://www.lua.org/ftp/)
was 5.4.9 (2026-08-10), verified 2026-11-16. Source is currently external;
fetch and verify it before the offline build:

```sh
mkdir -p .tmp/lua-probe
curl -fL https://www.lua.org/ftp/lua-5.4.9.tar.gz -o .tmp/lua-probe/lua-5.4.9.tar.gz
echo '2335b6c582a52654f94612bf10d2f4672805d05329aa6568b1d8cd9e5c6fb8e6  .tmp/lua-probe/lua-5.4.9.tar.gz' | sha256sum -c -
cmake -S front -B target/front
cmake --build target/front --parallel 3
python3 rt/musl/build.py --jobs 3
cargo build -p bennett -j 3
python3 experiments/lua-probe/build.py .tmp/lua-probe/lua-5.4.9.tar.gz
python3 experiments/lua-probe/run.py target/lua-probe
```

Wrap builds/runs with the shared machine's heavy-job limiter when applicable.
These experiments are opt-in and outside `cargo test`. All generated sources,
bitcode, device text and raw measurements stay under ignored `target/lua-probe/`.
The build re-extracts the checksum-verified archive; it never edits Lua sources.

Configuration: upstream generic C99 build with `LUA_COMPAT_5_3`, default 64-bit
integers and double-precision numbers; no POSIX, dynamic loading or readline
build flags. All standard libraries are registered by `luaL_openlibs`.
Native uses clang 18 `-O2`, host libc and libm. Device input uses clang 18 `-O0
-disable-O0-optnone`, musl 1.2.5 and the front's normal O2 pipeline. Vectorizers
are disabled on both sides. Lua bitcode members are appended to a probe-local
view of musl's archive index, so the existing `rt/musl/link.py` resolves them.
The interpreter and Lua compiler execute on the device, including parsing the
embedded source string. The tiny embedding driver only selects, loads, calls,
prints an uncaught error if any, and closes a Lua state. Its native main accepts
a case index; the hand-written classical device entry selects that same case
before entering normal musl startup. Counts include selection and entry glue.

## Stage 1: ordinary execution

Measured on base `229d3b9`, clang 18.1.3, 2026-11-16. All seven scripts return
status zero and identical stdout. Lua's unmodified setjmp-based protected error
handling is exercised by `pcall`, `xpcall` and coroutine resume. The default 1 MiB
musl arena and 1 MiB stack suffice; minima have not been measured. Lua's internal
hash/random seeds use its normal time/address code, so native/device seed bytes
differ; the scripts do not print pointers, iterate hash tables or draw randomness.

| Script | What it exercises | Forward steps |
|---|---|---:|
| arithmetic | Integer/float arithmetic, floor division, modulo, shifts | 759,889 |
| closures | Captured mutable upvalues and independent closures | 810,786 |
| coroutines | Yield/resume, return and coroutine status | 835,707 |
| errors | Caught errors, successful pcall and xpcall handler | 839,108 |
| format | Integer, decimal, scientific and hexadecimal float formatting | 820,153 |
| strings | Substrings, reverse and substitution | 757,628 |
| tables | Array sort, concatenation, fields and length | 784,748 |

No compiler refusal, device fault, wrong result or crash. Full link/lowering
takes 29.507 s; Lua compilation, native link and device link together 41.454 s
(cold front/musl/CLI builds excluded). Lowered device text is 3,345,350 bytes.

## Stage 2: full reversal

```sh
python3 experiments/lua-probe/run.py target/lua-probe --reverse
```

All seven stdout/status comparisons still agree. After un-running every step,
the CLI's independently captured entry snapshot compares equal in classical
memory, frames, slots, stack and console tape, with quantum distance exactly 0.
All seven start and end with one classical world and no quantum registers.
Forward and reverse step counts are identical, including the non-local jumps.
Journal bytes are reported directly; no byte/word conversion is assumed for
Lua's setjmp side tables.

| Script | Forward steps (also steps un-run) | Journal bytes |
|---|---:|---:|
| arithmetic | 759,889 | 14,706,593 |
| closures | 810,786 | 15,685,218 |
| coroutines | 835,707 | 16,201,155 |
| errors | 839,108 | 16,218,516 |
| format | 820,153 | 15,926,297 |
| strings | 757,628 | 14,653,004 |
| tables | 784,748 | 15,195,388 |

## Stage 3: a Lua predicate as a coherent oracle

```sh
python3 experiments/lua-probe/build.py .tmp/lua-probe/lua-5.4.9.tar.gz \
  --oracle --out target/lua-probe/oracle
cargo build -j 3 --offline --manifest-path experiments/lua-probe/harness/Cargo.toml --target-dir target
python3 experiments/lua-probe/coherent.py target/lua-probe/oracle
BENNETT_APPLY=compare python3 experiments/lua-probe/coherent.py target/lua-probe/oracle
python3 experiments/lua-probe/refusal.py target/lua-probe/oracle
```

`predicate.lua` defines `((n*n + 3*n + 1) % 7 == 4) and 1 or 0`. Native Lua
evaluates all inputs 0..7 and gives `[0,0,1,0,0,0,0,0]`. Classical setup opens
the Lua libraries, parses this source string and installs the function. Each
applied call uses `lua_getglobal`, `lua_pushinteger`, `lua_pcall`,
`lua_tointeger` and stack pop; the polynomial runs in Lua's VM. No C truth table
or precomputed bytecode substitutes for the interpreter. This is a separate
embedding, using the same unmodified 32 Lua sources, with a 3,345,640-byte device
module. It links/lowers in 28.584 s (37.194 s including Lua/native compilation).

The quantum glue is hand-written device text appended to that module. Every
ordinary Lua, libc and driver instruction is retained. Its width adapter is:

```text
func @lua_lookup2(i2 %input) -> i1 {
entry:
 %n = zext i2 %input to i32
 %value = call @lua_oracle_predicate(%n)
 %bit = trunc i32 %value to i1
 ret %bit
}
```

The entry calls classical `lua_oracle_setup`, allocates 2 input qubits, one
output qubit and the five-bit fault register, prepares the basis/uniform input,
then executes `apply @lua_lookup2(%input) -> %out, %fault budget 1000000`.
**All four basis inputs, uniform input with output initially 0 or 1, and two
applications followed by inverse Hadamards pass.** Every complex amplitude is
compared, including its sign, at tolerance 1e-12. The interference case frees
all registers cleanly. For each apply, the standalone observer captures its own
before/after snapshots and compares every memory byte, live slot, stack pointer,
console byte and frame (apart from the expected next instruction), checks a
zero fault register, and verifies no extra worlds or memory qubits remain.
Full reversal of every quantum entry also restores classical/world state exactly;
maximum amplitude distance from its initial snapshot is 2.23e-15.

| Basis input | Native bit | Region steps | Region journal bytes | Overlay bytes |
|---|---:|---:|---:|---:|
| 0 | 0 | 1,497 | 30,370 | 45 |
| 1 | 0 | 1,497 | 30,370 | 45 |
| 2 | 1 | 1,527 | 31,072 | 45 |
| 3 | 0 | 1,497 | 30,370 | 45 |

The N=4 uniform apply takes **6,018 region steps, 12,040 actual work steps**
including uncomputation/call overhead; region journal/overlay sums are
122,182/180 bytes, largest individual branch 31,072/45. Those are sums of
per-branch high-water marks, not simultaneously live storage. The complete
uniform entry takes 459,041 outer steps with 9,259,105 outer journal bytes in
the observer's reversible mode, peak 8 qubits and 4 configurations. All Lua state
is preserved immediately after apply. No region split copies are needed.

Grover uses the same Lua predicate, output prepared in `|->`, and diffusion
`D = 2|s><s| - I`. The extra X on the kickback qubit corrects diffusion's global
sign. The expected state comes from explicit dense oracle and diffusion
matrix-vector multiplication in Python, using the native Lua truth table;
it shares no gate sequence or executor arithmetic. All amplitudes, absent
configurations included, and normalization are checked. The output/fault
registers are freed cleanly; row amplitudes are inspected without measurement.

| N | Iterations | Expected marked probability | Maximum amplitude error |
|---|---:|---:|---:|
| 4 | 0 | 1/4 | 1.12e-16 |
| 4 | 1 | 1 | 2.23e-16 |
| 4 | 2 | 1/4 | 1.12e-16 |
| 4 | 3 | 1/4 | 1.12e-16 |
| 8 | 0 | 1/8 | 1.12e-16 |
| 8 | 1 | 25/32 | 3.34e-16 |
| 8 | 2 | 121/128 | 8.89e-16 |
| 8 | 3 | 169/512 | 6.67e-16 |

At N=8 peak use is 9 qubits and 16 configurations (kickback doubles support).
Each full-support Grover apply performs 8 region runs and 24,020 work steps,
with summed journal/overlay 243,662/360 bytes, zero copied bytes. Its
`region_steps` counter is 24,012 because that counter sums forward steps over
both output configurations; actual execution shares equal input arguments.
N=4 full-support Grover uses 4 runs and 12,040 work steps. Later sparse calls
can cost less. History and branch-enumeration comparison mode passes all
15 entries and every reversal, with the same amplitudes; comparison-mode cost
counters include both implementations and are not the cost table above.

### C marker finding and its route

`marker.c:11` asks for the same call with a qubit-dependent argument. Against
the identical linked Lua/musl bitcode, the front exits **3** in 10.584 s with:

```text
not handled yet: a call that depends on a qubit, to a function that writes memory or performs quantum operations
```

This is the same wall as the SQLite oracle: calling the interpreter writes Lua
stack/VM state and uses protected calls. The front's journalled fallback is the
named route past this refusal. The device already handles the call through
hand-written `apply`; no Lua, front or executor change is included here.

## Limits and validation

This is an optional probe, with a checksum-verified external source dependency
as in the C corpus. Lua is not vendored: the experiment does not add a production
runtime dependency. If it becomes one, retain the release sources unchanged,
its MIT licence and a VENDOR note as for the other runtimes.

Not measured: resource minima, allocation exhaustion, long-running programs,
arbitrary scripts, native/device random seed identity or random output, POSIX
or dynamic loading builds, the standalone interactive CLI/luac executables,
file-image scripts, coherent fault/budget boundaries, other Lua versions,
larger Grover sizes or different predicates. Register-based `apply`, `qalloc`
and `qfree` are intentionally used as permitted first-class device forms.
The oracle requires the front's fallback to be expressible through C markers;
this probe does not implement that route.

The observer is a standalone Cargo workspace and is not covered by the main
workspace's changed-line cargo-mutants discovery. A deliberate circuit mutation
removing diffusion's phase-correcting X was run: the amplitude checker rejects
`grover4_k1` with amplitude error **2.0** (+1 expected, -1 observed), despite
unchanged probabilities. This is a seeded sensitivity check, not an exhaustive
mutation audit of the observer or the Python scripts.
In a second temporary seed, the Rust observer prints `-amplitude.re` in place
of `amplitude.re`; Python rejects `basis0` with error **2.0**. Both deliberate
corruptions were caught; neither temporary source is part of this experiment.

The checked-in `scripts.tsv` and `coherent.tsv` retain exact result/cost rows;
raw native bytes and snapshot reports are in the ignored build directory.
The build's archive guard was also checked with corrupt bytes and rejected them
before extraction or compilation.

Required checks on the initial measurement base: `cargo test` **385 passed /
19 ignored**; `front/test.sh` **176 passed / 27 ignored**; Clippy with warnings
denied and formatting passed. The standalone observer also passed Clippy and
formatting.

Final validation after rebasing on `0c1be48`: `cargo test` **394 passed / 19
ignored**; `front/test.sh` **176 passed / 27 ignored**; Clippy with warnings
denied and formatting pass, including the separate observer. All seven script
comparisons/reversals and all 15 quantum entries in both history and comparison
modes pass again. Every checked-in TSV value reproduces exactly. The front and
musl sources are unchanged across this rebase; their lowered programs are reused
with the rebuilt executor. The marker refusal is reproduced (exit 3, 9.989 s).

Tooling finding: `python3 scripts/mutate-diff.py origin/main --list` on this
probe's committed diff fails with `JSONDecodeError` after cargo-mutants reports
`No mutants to filter`. Its workspace discovery excludes this standalone
observer, and it parses the tool's empty output as JSON before reaching its own
empty-set diagnostic. This was a scope preview, not a passing mutation run.
Route: handle cargo-mutants' empty output explicitly; standalone harness mutation
coverage remains a separate debt. The deliberate phase mutation above is the
measured sensitivity check.
