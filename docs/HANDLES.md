# The handle interface

This is the design of the C front end on *handles*, accepted on 2026-11-25. Qubits live in the device; C holds only
handles to them; every quantum operation is a call of an external function; and the one bridge from qubits to
ordinary code, `apply`, names a plain C function and the qubits it is applied to. It is built beside the marker
interface of `docs/MARKERS.md`, in new files, and replaces it once the milestone programs and Grover over SQLite
run in the new form.

Nothing in this document is built yet unless a section says so. The two lists at the end ("What the device must
provide", "What the runtime must provide") say what exists today and what the other parts of the tree must add.

Contents: 1 why handles; 2 the device as this design sees it; 3 handles; 4 programs and primitives; 5 the interface,
call by call; 6 `apply`; 7 step budgets; 8 libraries in ordinary C; 9 examples; 10 assumptions; 11 claims and their
status; 12 what is not claimed; 13 open choices; 14 what the device must provide; 15 what the runtime must provide.

## 1. Why handles

The marker interface lets a C variable hold qubits and lets ordinary statements touch them: `if (q) { ... }`,
`y = q + 3`. To translate those, the front end must decide for every value, store and statement whether it
*depends on a qubit*. Taken by the program text, that property is not something a C compiler preserves: two source
forms that differ in it can compile to the same instructions (merged stores, a guard another condition implies, a
choice between equal arms). Taken by value, it is undecidable. All but one of the known wrong results of the marker
front end are of this kind; the other is a program function mistaken for one of the front end's own by its name,
which section 4's declaration rule addresses.

The handle interface never asks the question. The programmer names every point where qubits meet ordinary code
with a call. What one compiled run does to the qubits is then defined by the external calls that run makes, in their
order and with their values, and by what each function it applies computes. Whether a compilation's calls follow the
C source, and whether two compilations agree, is a separate question: section 11 states what is proved about it and
under which premises (among them that the program's behaviour is determinate and that no resource runs out).

> **Rule R.** What a run does to the qubits is defined from the interface calls the compiled program makes and the
> functions it applies, and from nothing else. The front end translates each interface call from that call's
> run-time operands and the device's register table, and from nothing else.

Rule R restricts the *translation*. When a translated call runs, it also uses the allocation count (an allocation
returns the next handle), the memory its pointer operands point to (a control list, a field list, an image), and,
for an `apply`, a private copy of the whole ordinary state at the call (an applied function may read any global).

A change to the front end that needs to know anything about an interface call other than its operands and the
register table violates rule R. That is the test a reviewer applies.

That everything a qubit reaches becomes quantum stays the purpose: `apply` turns a whole C function, with its loops,
calls, memory and C library, into a unitary on the qubits it is applied to. Only the way the point of contact is
found changes: the program names it; the compiler never infers it. `if (q) { ... }` and `y = q + 3` are not part
of this interface (`docs/DESIGN.md`, decision of 2026-11-25); section 9 shows what they become.

## 2. The device as this design sees it

The device is the one of `docs/DESIGN.md`, "The model": a fixed target with its data layout, ordinary instruction
set, calling convention, limits and numbered fault codes. This section defines the terms the interface uses, in
the standard quantum circuit model.

- A **qubit** is a two-level system with basis states written `|0⟩` and `|1⟩`.
- A **register** of width `w` is a numbered group of `w` qubits at positions `0` to `w - 1`. Every allocation takes
  fresh qubits, so registers never share a qubit.
- A **basis configuration** assigns 0 or 1 to every qubit of every allocated register. In it a register *reads as*
  the integer `Σ_j 2^j b_j`, where `b_j` is the value of its qubit at position `j`: position 0 is the least
  significant bit.
- The **state space** is the tensor product of the two-dimensional spaces of all qubits of the allocated registers,
  ordered by handle and then by position. With no register allocated it is the one-dimensional space of complex
  numbers.
- The **quantum state** is a density operator on the state space: a positive operator of trace one. A pure state is
  the special case that assigns a complex **amplitude** to each basis configuration. A density operator is needed
  because a release (`bq_free`, below) leaves the remaining qubits in general in a **mixture**, which no single
  assignment of amplitudes describes.
- A **unitary** is a norm-preserving invertible linear map of the state space; a gate applies one, and the state
  `ρ` becomes `U ρ U†`, where `U†` is the conjugate transpose of `U`.
- The **partial trace** over a register removes its qubits from the state space without examining them; the other
  qubits keep the state they have without it.

Besides the quantum state the device keeps a little ordinary, definite state:

- the **register table**, which says for each handle whether it names an allocated register and how wide it is;
- the **allocation count** `a`, the number of successful allocations so far in the run, including registers
  released since; it starts at 0.

The device keeps nothing else between interface calls. The ordinary state of the control program (its current
instruction, call frames, machine registers, memory, stack, ordinary input and output) is separate from the
device's state. The **control program** is the ordinary program that issues the interface calls.

Two more terms name qubits:

- a **qubit reference** is a handle and a position;
- a **slice** is a handle, a lowest position `low` and a number of qubits `bits`; it names the positions
  `low` to `low + bits - 1` in increasing order and reads as `Σ_j 2^j b_{low + j}`.

## 3. Handles

A handle is the C value `bq_reg`, a struct with one `uint64_t` member `id`. The `k`-th successful allocation of a
run returns the handle whose `id` is `k`, counting from 1. A handle is never reused: after `bq_free` its integer
names nothing, and no later allocation returns it. The integer 0 is never a handle.

A handle is an ordinary value. It may be copied, stored, passed, returned and printed like any integer. C memory
never holds a qubit, and no C expression has a qubit's value; the only way from qubits to an ordinary value is the
outcome returned by `bq_measure` (and, inside an applied function, the bytes of an *in* field, section 6).

## 4. Programs and primitives

A **program** is a set of C translation units after preprocessing: the user's and those of the C library (musl),
all ordinary C. The **primitives** are the external functions a program calls and does not define: the seven
interface functions of section 5 (`bq_alloc`, `bq_free`, `bq_width`, `bq_measure`, `bq_gate`, `bq_rotate`,
`bq_apply`) and the device's four effect primitives: output of one byte (`__bennett_effect`), exit with a status
(`__bennett_exit`), and two named stops, `__bennett_out_of_memory` and `__bennett_bad_argument` (5.5). The header
`bq.h` declares all of them.

**The declaration rule.** In every translation unit of a program, the C library's included:

1. no global other than the header's declaration carries a primitive's *linker name* (the symbol the linker sees
   after every renaming the C dialect offers: an assembler label `__asm__("...")`, `#pragma redefine_extname`, an
   `alias` or `ifunc` attribute, a macro): no definition, alias, ifunc, variable or second declaration of it;
2. the header's declarations keep their linker names, their external linkage and every other property to the end
   of the unit: no pragma, attribute, macro or later declaration changes what a header declaration stands for;
3. every call of a primitive is the header's call: the header's function type and calling convention, and nothing
   on the call that the header's own call does not carry, except what a statement's context adds and says nothing
   about the callee (`#pragma STDC FENV_ACCESS`, `[[clang::nomerge]]`, `[[clang::noinline]]`,
   `[[clang::always_inline]]`, a loop pragma).

A declaration that emits nothing into the unit's code is accepted: one nothing uses, and one that appears only in
a type (an unevaluated array bound, an initializer a later one overrides). It cannot affect any call. A
redeclaration that makes the same call is accepted too. Without the first clause, a definition of `bq_free`, or an
alias or ifunc in its place, would put the program's own code in the primitive's place. Without the second, `-Dbq_free=ordinary_release` would rename the header's own declaration, and
every call of `bq_free` would go to an ordinary external function. Without the third, a redeclaration
`__attribute__((const)) uint64_t bq_width(bq_reg);` would let the compiler drop a call.

The rule is enforced before any optimisation, on what the compiler emits. The driver `bennett-cc` compiles each unit
with every LLVM pass off and with two files included by force ahead of the user's own includes: the header, then an
array of the eleven primitives' addresses. The compiler fixes the symbol a declaration stands for at its first use,
so the array shows the symbol each header declaration finally stands for. The driver compares each declaration and
each direct call of it with those of a *baseline* unit that only calls each primitive once, compiled with the same
command and options. It refuses by name, before any bitcode is written, every break of the three clauses. Calls
through a pointer are not refused: before optimising, the driver removes from them every attribute that is neither
ABI nor a limit on the optimiser and every metadata attachment other than debug information, which only costs their
optimisation. A pointer that reaches a primitive then makes that primitive's call whatever its declaration claimed.
The driver then deletes the array and lets the compiler optimise the same unit. After that, it checks only that no
global took a primitive's name and that every call of a primitive's declaration has its type and calling convention;
what the optimiser added is trusted. So is a claim the program makes about its own function: a GNU `const`
function that applies a gate is a false claim, undefined in C as natively, and the compiler may delete its call
with the gate inside, as natively. `bennett-front` makes the same check on every module it reads and refuses a
module that uses the interface without the driver's mark. This is the only compile-time refusal specific to
primitives. It looks at declarations and calls alone and asks nothing about what depends on what. Refusals of
ordinary C that the compiler's lowering makes still apply, as for any program. `docs/DESIGN.md`, "The declaration
check", states each rule with the measured behaviour of clang 18 it rests on.

Because the header is included everywhere, the C library's own files take their declarations of the effect
primitives from it and lose their own (section 15). The header declares the exit primitive `_Noreturn`, as the
library does today.

**How a primitive is recognised.** A primitive's linker name is its C identifier. The names are not names `clang`
knows as library functions, so no compiler pass treats them specially. The front end resolves the primitives itself,
after the last compiler pass: each primitive is one fixed device function at a reserved address, with no C body, and
a call whose target is that address, direct or through a function pointer, is that primitive call. The front end
translates the call from its operands alone; it does not look at where they came from.

A program compiled by stock `clang` for the device's target, at any optimisation level, is a **compilation**; the
front end **accepts** it if it translates it into a device program.

## 5. The interface, call by call

### 5.1 Conventions

- The device makes every check at run time, from definite values, in the order listed. The first check that fails
  ends the run with its named fault. Every value of every operand is either covered by a check or has the stated
  meaning.
- Every bound and byte count is computed as a mathematical integer, before any conversion to a machine integer: a
  slice with lowest qubit `2^64 - 1` and width 1 fails `low + bits <= width`; it does not wrap.
- A list of length 0 needs no readable byte, and its pointer may be any value. A list is *readable* if all of its
  bytes can be read as ordinary memory.
- A check marked † concerns what a pointer points to. At the source level, failing it is undefined behaviour (a
  program that fails it has no meaning in C); on the device it is a named fault like the others.
- A **control list** is a pointer `const bq_bit *controls` and a count `n`. Each entry is 16 bytes: the handle's
  `id` as an unsigned 64-bit integer at byte offset 0 and the qubit's position as an unsigned 64-bit integer at
  offset 8, little-endian, with no padding; the C type `bq_bit` is 16 bytes with alignment 8, and the header states
  its size, alignment and member offsets with `_Static_assert`. `n = 0` means no control. A qubit may occur twice in
  a list, which means the same as once.
- Gate codes and rotation codes are distinct numbers; this list is the one source the header and the device take them
  from: `BQ_GATE_X` 1, `BQ_GATE_Y` 2, `BQ_GATE_Z` 3, `BQ_GATE_H` 4, `BQ_GATE_S` 5, `BQ_GATE_SDG` 6, `BQ_GATE_T` 7,
  `BQ_GATE_TDG` 8; `BQ_ROT_X` 9, `BQ_ROT_Y` 10, `BQ_ROT_Z` 11, `BQ_ROT_PHASE` 12. A gate code given to `bq_rotate` is
  therefore `bad_gate`, and so is a rotation code given to `bq_gate`.

### 5.2 Faults

There is one numbering, the device's (`crates/ir/src/fault.rs`); the codes are 5 bits wide. A fault that ends the
run at a failed check is never written into a register. Where the device already has a fault with the same meaning,
the interface uses it.

| Interface name | Code | Device name | Raised by |
|---|---|---|---|
| `bad_function` | 4 | `bad_indirect` | `bq_apply`: `f` is not an applied function of the program |
| `bad_handle` | 9 | `dangling_register` | a handle that names no allocated register (never allocated, or released) |
| `control_in_use` | 10 | `qubit_alias` | a gate's target is also among its controls |
| `bad_angle` | 18 | `bad_angle` | a rotation angle that is not finite |
| `out_of_memory` | 19 | `out_of_memory` | the runtime's allocator cannot hold a request (`__bennett_out_of_memory`); a block `bq_apply` cannot place (6.3) |
| `bad_width` | 20 | new | `bq_alloc` with a width outside `1` to the device's limit |
| `out_of_handles` | 21 | new | `bq_alloc` when the allocation count has reached `2^64 - 1` |
| `bad_range` | 22 | new | a qubit position or slice outside its register; a fault register narrower than 5 |
| `bad_gate` | 23 | new | a gate or rotation code outside its set |
| `bad_argument` | 24 | new | an unreadable list or image, a malformed or overlapping field, a byte count that overflows; from the libraries of section 8, a scalar-form slice wider than 64 qubits and a `bq_pop` without a matching `bq_push` |

The meaning of `dangling_register` widens from "released" to "released or never allocated"; with handles as
integers both mean that the handle names no register.

The codes an applied function's run can end with, written into its fault register (6.4), are the device's codes of
ordinary code: `budget` 1, `divide_by_zero` 2, `out_of_bounds` 3, `bad_indirect` 4, `unreachable` 5,
`stack_overflow` 6, `effect_in_region` 7, `quantum_in_region` 8, `bad_stack_restore` 12, `exit_in_region` 13,
`bad_longjmp` 16, `qubit_memory_access` 17, `out_of_memory` 19, and `bad_argument` 24 when a library of section 8
stops inside an applied function. Codes 25 to 31 are free. The C header names them `BQ_FAULT_<NAME>`, with
`BQ_FAULT_EFFECT_IN_APPLY`, `BQ_FAULT_QUANTUM_IN_APPLY` and `BQ_FAULT_EXIT_IN_APPLY` for codes 7, 8 and 13.

### 5.3 Registers

**`bq_reg bq_alloc(uint64_t bits)`**

1. `1 <= bits <= ` the device's width limit (`bad_width`).
2. `a < 2^64 - 1` (`out_of_handles`).

Meaning: `a` becomes `a + 1`; the register table records handle `a` with width `bits`; the state `ρ` becomes
`V ρ V†`, where the isometry `V` adds `bits` qubits in state `|0⟩` (`V|z⟩ = |z⟩|0…0⟩`); returns the handle `a`.

**`void bq_free(bq_reg r)`**

1. `r` allocated (`bad_handle`).

Meaning: the partial trace over the qubits of `r`, whatever they hold. The handle is retired; `a` is unchanged. It
never faults on the register's contents.

**`uint64_t bq_width(bq_reg r)`**

1. `r` allocated (`bad_handle`).

Meaning: returns the width recorded for `r`, the `bits` of the allocation that made it. It reads the register table
only and does not act on the qubits.

**`uint64_t bq_measure(bq_reg r, uint64_t low, uint64_t bits)`**

1. `r` allocated (`bad_handle`).
2. `1 <= bits <= 64` and `low + bits <= width(r)` (`bad_range`).

Meaning: measures the slice `(r, low, bits)` in the basis. For each integer `t` below `2^bits`, let `Π_t` be the
projector onto the basis configurations in which the slice reads `t`. Outcome `t` has probability `tr(Π_t ρ Π_t)`
and, when that is positive, leaves the state `Π_t ρ Π_t / tr(Π_t ρ Π_t)`; the call returns `t`. The register stays
allocated. A measurement is never controlled: no form of the interface makes one depend on a qubit.

### 5.4 Gates and rotations

**`void bq_gate(uint64_t kind, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n)`**

1. `kind` is one of the eight gate codes (`bad_gate`).
2. `r` allocated (`bad_handle`).
3. `bit < width(r)` (`bad_range`).
4. The control list: †readable for `n` entries (`bad_argument`); then, for each entry in list order, its handle
   allocated (`bad_handle`), then its position below its register's width (`bad_range`).
5. The target `(r, bit)` is not in the control list (`control_in_use`).

**`void bq_rotate(uint64_t kind, double theta, bq_reg r, uint64_t bit, const bq_bit *controls, uint64_t n)`**

1. `kind` is one of the four rotation codes (`bad_gate`).
2. `theta` is finite (`bad_angle`).
3. Then as `bq_gate` from its check 2 on.

Meaning of both: the unitary that applies the gate's 2×2 matrix to the target qubit on the basis configurations in
which every control qubit is 1, and is the identity on the others. In the basis `|0⟩, |1⟩`: `X`, `Y`, `Z` are the
Pauli matrices; `H = (1/√2)[[1, 1], [1, -1]]`; `S = diag(1, i)`; `T = diag(1, e^{iπ/4})`; `SDG` and `TDG` are the
conjugate transposes of `S` and `T`. The rotations `BQ_ROT_X`, `BQ_ROT_Y`, `BQ_ROT_Z` are `exp(-i θ P / 2)` for the
Pauli matrix `P` of that axis, and `BQ_ROT_PHASE` is `diag(1, e^{iθ})`. A device with a finite gate set approximates
rotations by arbitrary angles.

**Why the phase rotation exists.** `Rz(θ) = e^{-iθ/2} · diag(1, e^{iθ})`: alone, the factor `e^{-iθ/2}` is a global
phase and cannot be observed. Under a control it is no longer global: controlled `Rz(θ)` and controlled
`diag(1, e^{iθ})` differ by the phase `e^{-iθ/2}` on the configurations where the control is 1. A routine written
with `Rz` as a phase gate is right alone and wrong when it runs inside a control scope (8.4). With `BQ_ROT_PHASE`
every gate of the interface is an exact matrix, not one up to a global phase, so a routine stays right under a
control.

### 5.5 The named stops

**`_Noreturn void __bennett_out_of_memory(void)`** and **`_Noreturn void __bennett_bad_argument(void)`**

No checks. Meaning: ends the current run of ordinary code with the named fault `out_of_memory` (19) or
`bad_argument` (24), as the device's `fault NAME` does. In the control program that ends the run. Inside an applied
function it ends that function's private run, and the code becomes the function's result in the fault register (6.4),
like `divide_by_zero`. Neither acts on the qubits.

The C library's allocator calls `__bennett_out_of_memory` when its arena cannot hold a request, so heap exhaustion
is a named fault (section 15). A request the C library rejects from its arguments alone, before it asks for any
storage (a `calloc` whose byte count overflows, a `malloc` of half the address space or more, an impossible
alignment), still returns a null result with its error code, exactly as natively. The libraries of section 8 call
`__bennett_bad_argument` for the checks they make themselves: a scalar-form slice wider than 64 qubits, and a
`bq_pop` without a matching `bq_push`; the scope library calls `__bennett_out_of_memory` when its list cannot grow
(8.4). There is no general primitive that raises any code: each named stop is its own primitive.

### 5.6 Output and exit

`void __bennett_effect(char byte)` appends one byte to the run's console output. `_Noreturn void
__bennett_exit(int status)` ends the whole run with `status`. Neither acts on the qubits. They are the primitives
the C library already uses (`docs/MARKERS.md`, "Device calls (not markers)"); this interface only moves their
declarations into the header.

### 5.7 Primitive calls inside an applied function

The first primitive call an applied function makes ends its private run (6.4): an interface function of 5.3, 5.4 or
`bq_apply` gives `quantum_in_region` (8), output gives `effect_in_region` (7), exit gives `exit_in_region` (13). The
code names the class only, not which function was called. It is given at the step that enters the primitive: the one
device instruction of the call, or, for a call through a pointer, the indirect call itself. It comes before any of the
call's checks: `bq_gate` with an invalid kind, or `bq_apply` with a function operand that fails 6.2's first check,
gives 8 inside an applied function, not `bad_gate` or `bad_function` (D14). That step counts as one step of the budget
(6.3) and ends the run with its code, so the call gives 8 also when it is attempted with one step of the budget left.
The two named stops of 5.5 are not in this list: they are ordinary code and give their own codes.

## 6. `apply`

### 6.1 Applied functions, images and fields

An **applied function** is a C function defined with the one type `bq_fn`, that is `void f(void *block)`.

```c
void bq_apply(bq_fn *f, uint64_t budget, bq_reg fault,
              const void *image, uint64_t size,
              const bq_field *fields, uint64_t m,
              const bq_bit *controls, uint64_t n);
```

The **image** is `size` ordinary bytes: the initial content of the **block**, the memory `f` receives. A **field**
names a slice of `w` qubits, a byte offset into the block and a direction, *in* or *out*. It occupies the
`ceil(w/8)` bytes from its offset. An *in* field's slice is written into its bytes before `f` runs; an *out* field's
bytes are read after `f` returns and XORed into its slice. A field entry in memory is 40 bytes, all unsigned 64-bit
little-endian integers, with no padding; the C type `bq_field` is 40 bytes with alignment 8, and the header states its
size, alignment and member offsets with `_Static_assert`:

| Offset | Member | Meaning |
|---|---|---|
| 0 | `slice.reg.id` | the handle |
| 8 | `slice.low` | the slice's lowest qubit |
| 16 | `slice.bits` | the slice's width, at least 1 |
| 24 | `offset` | the field's byte offset in the block |
| 32 | `direction` | 1 `BQ_DIR_IN`, 2 `BQ_DIR_OUT` |

A field list of `m` entries occupies `40 m` bytes; a control list of `n` entries `16 n` bytes. The **fault register**
`fault` takes the applied function's fault code in its five lowest qubits.

An **input** `x` is a list of definite values, one per *in* field, that gives a qubit occurring in several *in*
fields the same value in each. A field of more than 64 qubits is allowed: a 2048-qubit register is a field of 256
bytes.

### 6.2 Checks

In this order:

1. †`f` is the address of a function of the program whose compiled signature is one pointer parameter and no result,
   and not a primitive (`bad_function`). At the source level the check is that `f` designates a function of the
   program defined with type `bq_fn`.
2. `fault` allocated (`bad_handle`), then at least 5 qubits wide (`bad_range`).
3. `1 <= size <= ` the device's memory size, and the byte counts `40 m` and `16 n` do not overflow the target's
   byte-count arithmetic (`bad_argument`).
4. †`image` readable for `size` bytes and `fields` readable for `m` entries (`bad_argument`); then the control list
   as in 5.4.
5. For each field in list order (`bad_argument` throughout): `direction` is 1 or 2; its handle allocated;
   `bits >= 1`; `low + bits <= width`; `offset + ceil(bits/8) <= size`.
6. No two fields' byte ranges overlap; no qubit occurs in two *out* fields, in an *out* field and an *in* field, in a
   field and the five low qubits of `fault`, or in the control list and an *out* field or those fault qubits
   (`bad_argument`). A qubit may occur in several *in* fields, and in the control list and *in* fields.

### 6.3 The function `G` a device computes

At a call that passes the checks, the device determines a total function `G` from inputs `x` to an *out* value or a
fault code from 1 to 31:

1. **Placement.** Before any input is written, a fixed rule of the device chooses one base address for the block: a
   multiple of 16 at which `size` bytes are free, outside the program's heap and live stack. The rule reads `size`,
   where the heap and the live stack lie at the call, and nothing else: no other operand (not the budget, the fault
   register, the fields, the controls or the function) and no other memory content changes the base. The same base
   serves every `x`. If there is none, `G(x)` is `out_of_memory` for every `x`. (Which operands the base may read is
   not fixed by the decisions of 2026-11-25; this narrowest reading is the one easiest to widen, 13.)
2. **Snapshot.** The device copies the checked field list and the image at the call; the applied function cannot
   change them.
3. **Setup**, for each `x`, on a private copy of the whole machine state at the call, done by the device and not
   counted as steps: copy the image into the block; write each *in* field's value into its bytes, least significant
   byte first, with the unused high bits of the last byte 0; set the entry state of `f` (machine registers, stack
   pointer, the block's address as the argument) by the calling convention; the budget is not part of that entry
   state, and setup does not depend on it. If the private copy needs storage the device lacks, `G(x)` is
   `out_of_memory`; otherwise, if the stack has no room for the entry, `stack_overflow`. When both hold,
   `out_of_memory` is the result: the device fixes one order for coincident setup failures, and this document fixes it
   so.
4. **Run**, only when placement and setup succeeded: the compiled `f` for `budget` steps. A step is one instruction or
   terminator (a jump, call or return counts as one); a step that faults counts as one and ends the run with its code.
   A run that has returned or faulted stays so. After `budget` steps: if still running, `G(x)` is `budget`; if
   faulted, its code; if returned, extraction follows. So `budget = 0` gives `budget` for every `x` whose placement
   and setup succeeded; a failed placement or setup gives its own code whatever the budget.
5. **Extraction**, not counted: read each *out* field from its bytes, least significant byte first, reduced to its
   `w` bits. If an *out* byte cannot be read, `G(x)` is `out_of_bounds`. The private copy is discarded.

Steps 3 to 5 start from a state fixed by steps 1 and 2, and ordinary code is deterministic, so `G` is a function of
`x`. The fixed placement is needed for that: if two aligned bases were both allowed, a function that stores bit 4 of
its block's address in its *out* byte would give 0 or 1 for the same input.

The choices of 2026-11-25 that this section depends on:

- The block is placed before the input is written, at the same address for every input, aligned to 16 bytes. A
  function that uses the block as a type needing more than 16-byte alignment has no defined result at the source
  level.
- Addresses have one concrete layout: an address is the 64-bit index of a byte in the device's flat memory, and a
  function's address is its index plus 1 (`docs/DESIGN.md`, "Derived forms"). A pointer converted to an integer
  gives that number, which always fits a 64-bit integer; storage the C source treats as never exhausted meets the
  fixed width as the resource fault `out_of_memory`. The loader makes no free choice: the initial memory is the
  device program's data, and the stack starts at the top of memory.
- A quantum call inside an applied function is code 8, the device's `quantum_in_region` (5.7). One numbering
  serves the device language and the interface.

### 6.4 Meaning

With `G` as above, `bq_apply` is the unitary that maps every basis configuration in which all control qubits are 1 by

```
|x⟩ |o⟩ |e⟩  →  |x⟩ |o ⊕ out(G(x))⟩ |e ⊕ code(G(x))⟩
```

and every other basis configuration to itself. Here `x` is the value of the *in* fields' qubits, `o` that of the
*out* fields' qubits, `e` that of the five low qubits of `fault`, and `⊕` is bitwise XOR. If `G(x)` is an *out*
value, `out` is that value and `code` is 0; if it is a fault code, `out` is 0 and `code` is that code, never 0. A
qubit shared by inputs and controls is one tensor factor, not two. Every other qubit is unchanged.

Writes the applied function makes to memory do not survive the call: they happen on the private copy, which is
discarded. Only the *out* fields and the fault register change. A store that depends on a qubit still makes the
bytes qubits (decision of 2026-11-17); under this interface that happens in the private memory of an applied
function, which a device carries on qubits, never in the control program's memory.

**Reading the fault register.** Suppose the five low fault qubits are 0 before the `apply`. A measurement of them in
the basis gives a nonzero code with probability

```
p_fault = Σ, over the basis configurations in which every control is 1 and G(x) is a fault code,
          of their probability just before the apply,
```

in two cases: (a) the readout comes at once; (b) it comes later, is made on every continuation of the run (the
probability is then the average over all measurement outcomes in between), the fault qubits stay allocated, and
every operation in between preserves their basis values, outcome by outcome. Using the fault qubits as controls or
as *in* fields preserves them. If the readout is made only after some measurement outcomes, the statement fails:
with a control in the state `(|0⟩ + |1⟩)/√2` and a function that always faults, `p_fault = 1/2`, but reading the
fault only when a measurement of the control gave 0 never shows one. A program that XORs a code into fault qubits
that are not 0, or applies the same function to the same fault register twice, can clear a code; a fresh fault
register for each `apply` avoids that (8.2).

**The same in C.** For each input `x`, the call of `f` on `x` is what this C code does on a private copy of all
memory, with `b` a fresh object of `size` bytes at a multiple of 16, outside the program's heap: copy the image into
`b`; write the *in* fields; `f(b)`; read the *out* fields. The device does the same for the compiled `f`, with the
block placed by its fixed rule; only the steps of `f` count.

**How a device carries it out.** A device computes each step of `f` into fresh qubits that keep the whole previous
state, copies the result out, and runs the steps backwards (`docs/DESIGN.md`, "`apply`"; the model paper's
compute, copy and undo construction, Theorem 7.5, and its comparison of time with retained workspace, Theorem 8.3).
Every implementation of `apply` returns its scratch qubits to 0 (decision of 2026-11-25): an implementation that
realises the same map in another way, for example by flipping and then discarding an independent scratch qubit, is
not allowed.

## 7. Step budgets

`budget` is an ordinary value, computed by the program at run time like any other; it is one bound on all the
steps of the applied function, its loops and calls included, never a bound per loop. A program does not guess it.
It obtains it from a run that reports the steps an `apply` used:

- **The report.** In a *measuring run*, the device reports, for each `bq_apply` the run executes, in order: its
  position in the run (the `j`-th `apply`), the applied function's name, the number of inputs it evaluated, the
  largest number of steps any of them used to return or to fault with a code other than `budget`, and how many
  reached the budget. The measuring run gives its `apply` calls a ceiling as their budget (below).
- **Why that is enough.** A run that has returned or faulted stays so, and neither the block's base nor the entry
  state of the applied function depends on the budget (6.3). Hence, at the same `apply` of the same executable, with
  the same ordinary state at the call and the same operands other than the budget, any budget at least the reported
  largest count gives the same result as the measuring run on every input that finished there. That says nothing about
  an input that reached the ceiling: it has no count, and a larger budget can turn its `budget` fault into a return.
  Only when the report shows no input at the ceiling is the largest count a bound for every input the run evaluated;
  otherwise the measuring run is repeated with a higher ceiling, or the function does not end on that input. The
  ordinary state must really be the same: if the ceiling or the budget is part of what the applied function reads (a
  global set from the run input, say), changing it changes the state the guarantee is about, so a program keeps the
  budget out of what its applied functions read.
- **What it covers.** The inputs the measuring run evaluated (on the simulator, the basis configurations of nonzero
  amplitude at that call), on that compilation. Counts differ between compilations, for example between `-O0` and
  `-O2`, so a budget is measured for the compilation that will run. Grover's oracle sees every input of its
  register in the first iteration; for a function applied to a register in a narrower state, the measuring run
  should apply it once to a uniform superposition of its inputs.
- **How the program receives it.** The header names each budget: `BQ_BUDGET(name)` expands to the macro
  `BQ_BUDGET_name`, which the build defines, and no program writes a number. A *measuring build* defines every
  budget as unbounded, each name with its own value (`BQ_UNBOUNDED - i` for the `i`-th name, `BQ_UNBOUNDED` being
  `UINT64_MAX`), so that the report of each `apply` names its budget; the *result build* defines each name as the
  largest count reported for it. Both are builds of the same compilation flags, so the count applies to the
  compilation that runs. A function that never ends on some input cannot be measured without a bound: its measuring
  build gives it a ceiling, doubled from 1 until every `apply` with that budget has an input that ended within it.
  The alternative, a budget as a run input, needs run arguments under the C library (R6); a build-time definition
  needs nothing from the device but the report (D16), so it was taken (13).

The example programs of `examples/handles/` and the checks of `front/tests/handles/` run this way on a native
stand-in for the device (`front/tests/handles/standin.c`, a state-vector simulator; `front/tests/tests/handles_native.rs`
runs it). It is a comparison aid, not the device, and the device is the reference for everything it does. It
evaluates the applied function once per input in a forked child process, whose memory is a private copy, as on the
device. Its steps are its own count, not the device's: the entry into the applied function and each basic block of
program code entered inside it (the program is compiled with `-fsanitize-coverage=trace-pc-guard,pc-table,no-prune`
and the stand-in supplies the callbacks; the C library's own code is not counted), so a budget measured there is a
budget for the stand-in only. The budget is the program's; the stand-in has no bound of its own. After each `apply`
it writes `bq: apply <k> budget <b> inputs <e> steps <s> out-of-budget <n>` to standard error; the test measures with
every seed, builds again with the counts, requires the result runs to give the measuring runs' output, and checks that
one step less makes an input run out.

The stand-in claims agreement with the device for one class of applied functions, and for nothing else: functions
that compute on their block and on ordinary memory (allocating and releasing it included, every allocation
succeeding), call no effect, exit or interface primitive, register no handler, do no buffered output, give out values
that do not depend on numeric addresses, and return. Every applied function of the five programs and of the library
checks is in it. Outside it a result, a fault code included, says nothing about the device's; for example, each of
these gives a result natively that the device does not: an exit handler registered inside the applied function,
buffered output to a file stream, fork callbacks of the program (they run at the stand-in's fork, before the applied
function reads memory), output the program left buffered before the `apply` (the stand-in flushes standard output and
error at each `apply`), and an explicit trap (natively a signal, reported as `out_of_bounds`; on the device
`unreachable`). Some calls outside the class stop the run with `bq: limit <what> in <call>` instead (a system call
other than those that map or unmap anonymous memory or end the process, a shared mapping at the `apply`, an `f` that
is not a function entry of the program's code, the simulator's own storage running out), but that is a help, not a
check of the class: a call outside it may also go unnoticed.

A smaller budget makes the circuit a device builds for an `apply` smaller, so a measured budget is also a cost
measure. Today the simulator reports only the sum of all steps inside applied functions (`region steps`); the
per-`apply` report is in section 14.

## 8. Libraries in ordinary C

Everything in this section is ordinary C in the header or in a library file that calls the interface. Its meaning is
C's, and it adds nothing to the device. Shorter notation may be added in two ways only: a library in ordinary C, or a
form defined by expanding it at the source level, before any compiler pass, by a rule that looks at that form alone
(a C macro is the model case). A form whose expansion needs to know what another statement does, or what a value
depends on, is not admissible.

### 8.1 Slices and fields

```c
static inline bq_bit   bq_bit_of(bq_reg r, uint64_t bit)                  { return (bq_bit){ r, bit }; }
static inline bq_slice bq_slice_of(bq_reg r, uint64_t low, uint64_t bits) { return (bq_slice){ r, low, bits }; }
static inline bq_slice bq_all(bq_reg r)                                   { return bq_slice_of(r, 0, bq_width(r)); }

static inline bq_field bq_field_of(bq_slice slice, uint64_t offset, uint64_t member_size, uint64_t direction) {
    if (slice.bits / 8 + (slice.bits % 8 != 0) > member_size) __bennett_bad_argument();
    return (bq_field){ slice, offset, direction };
}
#define BQ_MEMBER_SIZE(type, member)  sizeof(((type *)0)->member)
#define BQ_IN(slice, type, member)    bq_field_of((slice), offsetof(type, member), BQ_MEMBER_SIZE(type, member), BQ_DIR_IN)
#define BQ_OUT(slice, type, member)   bq_field_of((slice), offsetof(type, member), BQ_MEMBER_SIZE(type, member), BQ_DIR_OUT)
#define BQ_IN_ALL(reg, type, member)  BQ_IN(bq_all(reg), type, member)
#define BQ_OUT_ALL(reg, type, member) BQ_OUT(bq_all(reg), type, member)
```

The functions exist because a compound literal such as `(bq_slice){ r, 0, 4 }` is split at its commas when it is a
macro argument. `bq_all` asks the register its width, so a whole-register field needs no restated width.

A field's bytes and its member's type are separate things: a field writes and reads `ceil(w/8)` bytes, least
significant first. A 4-qubit field on a `uint32_t` member fills its low byte and leaves the other three as the image
had them; a 4-qubit *out* field on a `uint8_t` member drops bits 4 to 7 of whatever `f` stores there. The macros
check one thing more than `bq_apply` does: that the field's bytes lie inside the member it names. A 40-qubit field
on a `uint32_t` member would otherwise write into the next member, which `bq_apply` cannot see; it stops with
`bad_argument` instead. The example programs use `uint64_t` members, which hold any slice of up to 64 qubits.

### 8.2 Fault registers

```c
static inline bq_reg bq_fault_alloc(void) { return bq_alloc(BQ_FAULT_BITS); }      /* BQ_FAULT_BITS is 5 */
static inline uint64_t bq_fault_take(bq_reg fault) {                               /* measure, release, return */
    uint64_t code = bq_measure(fault, 0, BQ_FAULT_BITS);
    bq_free(fault);
    return code;
}
const char *bq_fault_name(uint64_t code);                                          /* "budget", ... */
```

`bq_fault_take` is a measurement, with the probability of 6.4. It is safe at a point where the code is 0 with
certainty, or at the end of a computation; in the middle of an algorithm, a fault on part of the state is a
measurement outcome like any other and changes the state. Nothing in the interface stops the run if any amplitude
faulted without measuring.

So that a fault register costs one argument at the `apply` and one line at the point the program chooses to read
it, the header keeps a set of fault registers waiting to be read:

```c
bq_reg   bq_fault(void);                    /* bq_fault_alloc(), added to the set                       */
uint64_t bq_faults_take(void);              /* bq_fault_take of each, oldest first; the first nonzero code */
void     bq_faults_check(const char *where);/* bq_faults_take; on a fault: "fault <name> in <where>" on
                                               standard output, exit(BQ_FAULT_EXIT_STATUS), which is 4  */
```

The message goes to standard output because the device's runtime has a console only: a write to standard error
stops the run there. An oracle is then `bq_apply_scoped(f, BQ_BUDGET(f), bq_fault(), ...); ...;
bq_faults_check("oracle");`. Reading
later costs qubits (five per `apply` until the check); reading sooner measures sooner. The set is ordinary memory, one
per program (8.4), grown on demand.

### 8.3 Scalar forms

For a C function `g` of `k` integers, `k` from 0 to 4, the header defines a wrapper of type `bq_fn`, its image
struct, and a builder. For `k = 1`:

```c
struct bq_w1 { uint64_t (*g)(uint64_t); uint64_t a0, r; };
static void bq_w1(void *p) { struct bq_w1 *w = p; w->r = w->g(w->a0); }

static inline void bq_apply_1(uint64_t (*g)(uint64_t), uint64_t budget, bq_reg fault,
                              bq_slice out, bq_slice in0, const bq_bit *controls, uint64_t n) {
    if (out.bits < 1 || out.bits > 64 || in0.bits < 1 || in0.bits > 64) __bennett_bad_argument();
    struct bq_w1 image = { g, 0, 0 };
    bq_field f[2] = { BQ_IN(in0, struct bq_w1, a0), BQ_OUT(out, struct bq_w1, r) };
    bq_apply(bq_w1, budget, fault, &image, sizeof image, f, 2, controls, n);
}
```

For other `k` the scheme is the same: `k` zero-initialised `uint64_t` members `a0` to `a(k-1)` and one zero-initialised
result member `r`; the wrapper stores in `r` the value of `g` on the members in order (`w->g()` for `k = 0`, whose only
field is the output); the builder takes `k` input slices after `out`, checks that every slice has at most 64 qubits,
and passes `k + 1` fields, the *in* fields at the members' offsets followed by the *out* field at `r`. So `out ^= g(in0,
...)` on each basis configuration where the controls are 1.

The explicit width check is the choice of 2026-11-25: without it the rejection of a wide slice would rest on the
layout of the image struct (a 65-qubit field overlaps the next member only when the members are contiguous 8-byte
integers). A zero-width slice stops there too (it would be `bad_argument` of `bq_apply` anyway).

The budget is the wrapper's: it counts the wrapper's own steps and those of `g`.

### 8.4 Control scopes

The device has no control stack. A scope "every gate from here to there is controlled by `c`" is a small library in
ordinary C: it keeps the current list of control qubits in ordinary memory, and its wrappers pass that list to the
calls of 5.4 and 6. On its successful paths:

```c
static bq_bit  *active;                 /* one list per program: see below */
static uint64_t depth;

void bq_push(bq_reg r, uint64_t bit) {
    reserve_controls(depth + 1);          /* grows the storage on demand, keeping the old entries;
                                             stops with out_of_memory if it cannot */
    active[depth++] = (bq_bit){ r, bit };
}
void bq_pop(void) { if (depth == 0) __bennett_bad_argument(); --depth; }

uint64_t      bq_scope_depth(void) { return depth; }
const bq_bit *bq_scope_list(void)  { return active; }

void bq_x(bq_reg r, uint64_t bit)                { bq_gate(BQ_GATE_X, r, bit, active, depth); }
void bq_phase(double t, bq_reg r, uint64_t bit)  { bq_rotate(BQ_ROT_PHASE, t, r, bit, active, depth); }
void bq_apply_scoped(bq_fn *f, uint64_t budget, bq_reg fault, const void *image, uint64_t size,
                     const bq_field *fields, uint64_t m) {
    bq_apply(f, budget, fault, image, size, fields, m, active, depth);
}
void bq_apply_scoped_1(uint64_t (*g)(uint64_t), uint64_t budget, bq_reg fault, bq_slice out, bq_slice in0) {
    bq_apply_1(g, budget, fault, out, in0, active, depth);
}
```

and likewise `bq_y`, `bq_z`, `bq_h`, `bq_s`, `bq_sdg`, `bq_t`, `bq_tdg`, `bq_rx`, `bq_ry`, `bq_rz` and
`bq_apply_scoped_0` to `bq_apply_scoped_4`. With no scope pushed the list is empty and every wrapper issues its call
uncontrolled, so the wrappers are also the short forms for plain gates.

The list is last in, first out: `bq_pop` removes the most recent entry still outstanding. Its failures are named
stops (decision of 2026-11-25): a pop without a push stops with `bad_argument`. The list has no fixed capacity:
`reserve_controls` grows its storage through the C library's allocator. A request the arena cannot hold is the
allocator's `out_of_memory`. The library also computes the new capacity and its byte count with checked arithmetic,
and when the count does not fit a 64-bit integer, or the allocator returns a null result for a positive request
(which it does for a request it rejects from its arguments alone, 5.5), it calls `__bennett_out_of_memory` itself
before writing any entry. So every failure to grow is `out_of_memory` (19), and no new code exists for it.

A routine that issues its gates through the wrappers can be used under control unchanged, provided it uses
`bq_phase` (not `bq_rz`) where it means a phase (5.4). Because the list is ordinary state of the program, what a
return, `longjmp` or `exit` does to it is plain C: none of them pops. A measurement, allocation or release inside a
scope is, as always, not controlled. A routine that tests `bq_scope_depth()` can behave differently under a scope;
that is ordinary C too (11, claim 8).

The header defines this library inline. Its state, the list and its depth (and the fault set of 8.2), is a weak
definition in the header, which the linker merges into one object for the whole program, so that a routine in
another translation unit sees the scope it is called under; no library file is needed. Storage grows by doubling
through `realloc`, with the checks above (the fault set's too). Two short forms add one control to one gate: `bq_cx(c, cb, r, b)` and
`bq_cphase(theta, c, cb, r, b)` are a push of qubit `cb` of `c`, the gate on qubit `b` of `r`, and the pop.

## 9. Examples

The programs in `examples/handles/` are written in this interface: the thesis program, Grover over a C predicate,
Shor for 15 and for 21 (`shor.c` with the number and base defined by `shor15.c` and `shor21.c`), and Grover over
SQLite. Both Grover programs take their diffusion step, the exact inversion about the mean, from `grover.h`; it is
written with the scope wrappers and checked under a control by `front/tests/handles/library.c`. The programs run on
the native stand-in of section 7 and give the same output at `-O0` and `-O2 -fno-finite-loops`
(`front/tests/tests/handles_native.rs`; Grover over SQLite in an ignored test that needs the SQLite sources). The
fragments below show the forms.

The thesis program: put a qubit in superposition, let ordinary code depend on it, measure.

```c
#include <bq.h>

struct io { uint8_t q, r; };
static void negate(void *p) { struct io *io = p; io->r = !io->q; }   /* plain C */

uint64_t thesis(uint64_t budget) {
    bq_reg q = bq_alloc(1), r = bq_alloc(1), fault = bq_fault_alloc();
    bq_h(q, 0);
    struct io image = { 0 };
    bq_field f[] = { BQ_IN_ALL(q, struct io, q), BQ_OUT_ALL(r, struct io, r) };
    bq_apply(negate, budget, fault, &image, sizeof image, f, 2, 0, 0);           /* r ^= !q */
    if (bq_fault_take(fault) != 0) return 4;
    uint64_t qv = bq_measure(q, 0, 1), rv = bq_measure(r, 0, 1);                 /* always rv == !qv */
    bq_free(q);
    bq_free(r);
    return qv << 1 | rv;
}
```

A CNOT and a Toffoli, as scalar forms, as gates and through a scope:

```c
static uint64_t id(uint64_t a)               { return a; }
static uint64_t both(uint64_t a, uint64_t b) { return a && b; }
bq_apply_1(id,   budget, fault1, bq_slice_of(t, 0, 1), bq_slice_of(a, 0, 1), 0, 0);                        /* t ^= a */
bq_apply_2(both, budget, fault2, bq_slice_of(t, 0, 1), bq_slice_of(a, 0, 1), bq_slice_of(b, 0, 1), 0, 0); /* t ^= a && b */
bq_gate(BQ_GATE_X, t, 0, &(bq_bit){ a, 0 }, 1);       /* the CNOT as a gate */
bq_push(a, 0); bq_x(t, 0); bq_pop();                  /* the same through a scope */
```

Grover's oracle over any C predicate, here a SQLite query; the database pointer is ordinary data in the image, and the
query runs on the private copy of memory for each key:

```c
int row_matches(sqlite3 *db, uint32_t key);   /* plain C over the SQLite API: prepare, bind key, step, finalize */

struct query { sqlite3 *db; uint32_t key; uint8_t hit; };
static void matches(void *p) { struct query *q = p; q->hit = row_matches(q->db, q->key) != 0; }

struct query image = { .db = db };
bq_field f[] = { BQ_IN_ALL(key, struct query, key), BQ_OUT_ALL(flag, struct query, hit) };
bq_apply(matches, budget, fault_a, &image, sizeof image, f, 2, 0, 0);   /* flag ^= matches(key) */
bq_z(flag, 0);
bq_apply(matches, budget, fault_b, &image, sizeof image, f, 2, 0, 0);   /* clears flag */
```

What the marker interface's inferred forms become:

| Marker interface, inferred | Handle interface, named |
|---|---|
| `r ^= f(q);` with `q` a qubit variable | `bq_apply_1(f, budget, fault, bq_all(r), bq_all(q), 0, 0)` |
| `y = q + 3;` with `y` ordinary memory | `y` is a register; a scalar form of a function adding 3 |
| `if (q) { ordinary statements }` | the statements are an applied function with `q` as an *in* field |
| `if (q) { gates }`, `if (q) for (...) gate` | `bq_push(q, 0); gates; bq_pop();`, or explicit control lists |
| `if (q) measure(...)` | no form: a measurement is never controlled |

A guard on ordinary values stays ordinary C: in `if (n) for (j = 0; j != 1; j += n) bq_x(y, 0);` under a scope,
`if (n)` is a test of an ordinary integer, so for `n = 0` no permitted source execution enters the loop, and an
accepted correct compilation does not enter it either (11, claim 3, under its premise).

## 10. Assumptions

None of these is proved; each claim of section 11 says which it uses.

- **A1, the compiler.** A compilation is *correct* if, for every C file that defines the primitives and every run
  input on which the source program linked with that file reaches no undefined behaviour, the compiled program linked
  with the same file behaves as the source permits: the same calls of the file's functions with the same observable
  operands in the same order, the same outcomes of the file's calls back into the program, and the same ending or
  running forever; or the same up to a point where it stops because the stack or memory is exhausted. No assumption
  is made that a loop ends (`-fno-finite-loops`). This is the correctness of separate compilation, on which linking
  C rests, supported by experiment for `clang` and what verified compilers prove. It is not a sentence of ISO C.
- **A2, the device carries out the operations of sections 5 and 6**; for `apply`, by the compute, copy and undo
  construction or another that realises the same unitary and returns its scratch qubits to 0 (6.4).
- **A3, ordinary code on the device is deterministic**: every ordinary step is a total function of the machine state,
  with named faults where C is undefined. Completing a primitive call and the setup and extraction of an `apply` are
  not ordinary steps; sections 6.3 and the device conventions below fix them.
- **A4, the device runs compiled code faithfully.** For every list of measurement outcomes, the device run makes the
  calls, in order and with the observable operands, that the compiled program linked with a C file giving exactly
  those answers makes on the target, and ends alike, up to a resource fault; and for every `apply` and input, the
  device's run of the compiled `f` has the outcome that the compiled `f` has when called from such a file, up to a
  resource fault. On the device, a request the heap's arena cannot hold is the fault `out_of_memory`; a request the
  C library rejects from its arguments alone, before it asks for any storage, returns a null result with its error
  code as natively (5.5, section 15).

**Device conventions** (part of the design, not assumptions about `clang`): the initial ordinary state is a function
of the executable and the run input; after a primitive call succeeds, the control program's next state is a fixed
function of its state at the call and the answer, and the device writes no memory of the program; and the placement,
setup, counting and extraction of an `apply` are as in 6.3.

**Events and records.** The claims compare runs by these labels.

- An *event* is a primitive call. Its *observable form* is the primitive's name and its ordinary operands: a handle
  as its integer, a number by value, a list by its length and its entries in order. For `bq_apply` it is the budget,
  the fault handle, the size, the fields and the controls, but not the function, no address and not the image bytes.
  An output records its byte, an exit its status.
- The *record* of a successful call is its observable form. At the source, the record of a successful `bq_apply`
  also carries the domain `D` and the source function `F` of that call (defined below), and two such records are
  equal only if their domains are equal and their values on the domain are equal. On the device a successful
  `bq_apply` determines `G` instead (6.3).
- The *terminal record* of a call that fails a check is its name, every scalar operand of its observable form (list
  lengths included; no address, so two empty control lists at different addresses give the same record), one slot per
  list, and the fault of the first failed check. A slot holds the list's entries if they exist as values in the state
  before the call (every list the checks read, and every empty list), and the mark *unread* otherwise. Forming the
  record runs no check the call skipped and reads through no invalid pointer: a gate with an invalid kind and a null
  control list of one entry has a terminal record with the slot *unread* and the fault `bad_gate`.
- A run's *ending* is an exit with its status, a terminal record, a stop of ordinary code with its fault, or *silent
  divergence*: running forever with no further event (also before the first event).
- A comparison of a compiled run with a *reference* (a source execution, or a compiled run linked with a C harness)
  is *directional*: the compiled run either agrees with the reference, with equal records, answers and ending,
  through the part compared, or agrees on a common first part and then stops for lack of stack or memory. A resource
  stop of the reference alone gives no licence to a compiled run that goes on.

**Premises** that some claims add, each a statement that a certain C test harness exists and that A1 and A4 hold
when read through it:

- **P1, a passive witness.** A C file defining the primitives with the header's signatures, linkable with the program,
  that answers a fixed script (the next handle for an allocation; for a measurement the scripted value if it is in
  range and 0 otherwise; for a width query the width given at that register's allocation; for output and every other
  successful call a plain return with no other effect), makes the checks of sections 5 and 6.2 in order, keeps its
  state out of the program's reach, touches none of the program's objects, is defined C on every source execution, and
  at an exit or at the first failed check stays in an endless loop inside that primitive. The checks marked † (a list
  or image readable, `apply`'s function operand) are undefined behaviour at the source when they fail: the witness
  reads a list only when its check is reached on a path the source defines, and need not detect an invalid pointer.
  Every other failed check is a defined ending with its fault, at the source as on the device. Comparisons of a
  compilation with the source are made through the witness, with records and terminal records as above, its loop read
  as the call's ending (its status or first fault), any other running forever with no further event read as silent
  divergence (an endless sequence of events is an infinite run, not an ending), and every comparison directional.
  Where a finite script is continued, the device conventions above and A3 fix the ordinary state; a whole infinite run
  needs one witness for all of it.
- **P2, an observer for one `apply` and one input.** For a successful `apply` and an input `x` in its domain: a C
  file defining the primitives, linkable with the program and the C library, that answers the calls before that
  `apply` as scripted and loops on anything unexpected, holds outside the heap a C object of exactly `size` bytes at
  a multiple of 16 whose type and lifetime are compatible with every access the applied function makes on `x`,
  copies the image and writes `x` into it, runs `f`, loops at the entry of any primitive `f` calls, reads the *out*
  bits when `f` returns, and never lets the program resume. (An aligned byte array does not in general give the right
  to access it as a struct, so this is a real restriction.)
- **P3, the comparison through that observer.** At the same `apply` and `x`: a P1 witness with the same answers up
  to that `apply` covers the comparison run; A1 compares, in one and the same source behaviour, the run before the
  `apply` and what the observer sees (a return with its *out* values, the first primitive entered, by name, or running
  forever in `f`); for an `x` whose `G(x)` is not a resource fault, the compiled program linked with the observer
  reaches that call of `f` and is not cut by a resource stop before what the observer sees; and the comparison with
  the device is directional.
- **P4, the whole tree of outcomes.** P1 for every finite script of answers, including those of probability zero,
  and for every ending, including silent divergence before the first event; P2 and P3 at every successful `apply`
  and every input in its domain.
- **P5, a scripted comparison for a finite circuit.** For each finite path of the circuit, a C file that gives that
  path's answers, leaves the program's objects alone, returns from output and loops in the exit primitive. Through
  it, for every history of answers (including one of probability zero), while the earlier calls and answers agree,
  the next call of the compiled program and the next device event agree in observable form, unless either side first
  stops for lack of resources; compared up to the entry into the exit primitive, whose status the device records.
- **P6, faithful resolution.** The front end resolves each function defined with type `bq_fn` to its own address,
  with the compiled signature of 6.2's first check, so a call that passes the source check passes the device check.
  This is not automatic: the device signature does not tell a pointer parameter from a 64-bit integer (D11).

**The domain of an applied function.** For an `apply` at a given point of the source program, its *domain* `D` is the
set of inputs on which the call of `f` (6.4, "The same in C"), up to its return or its first primitive call, reaches no
undefined behaviour, does not leave `f` by `longjmp` to a point saved outside the call, and has one outcome under every
choice C leaves unspecified. On `D`, `F(x)` is that outcome: a return with *out* values, running forever, or a
primitive call. `G` *agrees with* `F` on `x` if `G(x)` is the returned value, or the class code of that primitive
(5.7; for a named stop of 5.5, its own code), or `budget` for running forever.

A program is *determinate* on a run input if, for every list of answers, no permitted execution reaches undefined
behaviour, and all permitted executions give one and the same tree: the same records (so, at each successful
`bq_apply`, the same `D` and `F`) after the same answers, and the same endings. The function is part of it because
equal calls are not enough: two arguments of one call, evaluated in either order as C permits, can set a global
byte to 0 and to 1; an applied function that copies that byte into its *out* field then makes the same calls write
different bits.

## 11. Claims and their status

The proofs are not yet published. This section states what they prove and under which premises, and no more.
*Proved* means that a written proof of the statement as given here exists; *open* means that none does. Each claim
names the assumptions and premises it uses.

| # | Claim | Status |
|---|---|---|
| 1 | No ordinary value depends on the quantum state except through measurement outcomes | proved (A3, device conventions) |
| 2 | Every device run has a meaning | proved, given the fixed placement, setup and extraction of 6.3 (A3), for the forms of the interface without the additions of claim 12 |
| 3 | The records of a correct compilation follow a path the C source permits | proved under P1 (A1, A3, A4) for every finite observation, up to a resource stop of the compiled run; open for a whole infinite run, where it does not follow from A1 to A4 |
| 4 | Two correct compilations of a determinate program have the same records | proved under P1 for each (A1, A3, A4), up to the first resource stop |
| 5 | Each applied function agrees with C on every input of its domain, unless it gives a resource fault | proved for determinate programs under P1, P2, P3 and P6 (A1, A3, A4); open without them |
| 6 | Two correct compilations have the same meaning | proved under P4 and P6 (A1, A3, A4), for determinate programs with full domains and no resource fault on any history or input |
| 7 | `apply` is unitary and leaves nothing behind | proved (A2, A3 where stated) |
| 8 | Control behaves as a circuit's control | proved (A2, A3 where stated) |
| 9 | Nothing computable is lost | proved under P5 (A1, A2) for finite circuits; proved for applied functions (A2, A3); proved for the scalar forms under the conditions stated |
| 10 | No program is refused for how it uses the interface | proved as a requirement on the front end, with C identifiers; open with linker names |
| 11 | The known wrong results of the marker front end have no counterpart | proved for the forms stated, assuming the declaration rule with linker names; comparisons across compilations under the premises of claims 3 to 5 |
| 12 | The additions of 2026-11-25 keep claims 1 to 11 | open |

**Claim 1.** For a fixed compiled program and device, after the `k`-th event of any run that reaches it, the ordinary
state of the machine and of the device is a function of the run input and the outcomes of the measurements among the
first `k` events. The device writes no program memory, and its answers are functions of its ordinary state and the
outcomes.

**Claim 2.** For every compiled program, run input and list of outcomes, each call the run reaches is exactly one
operation (an allocation, a unitary, a measurement, a partial trace, an output or exit) or exactly one checked fault
that ends the whole run. Every such fault is decided from ordinary values at the call, so it is an event of the run,
never a property of part of a superposition. A fault inside an applied function is a value of `G`, not a stop. For
each `j`, the probabilities of the distinct lists of `j` outcomes sum to at most 1. This needs the fixed placement of
the block and a setup and extraction that each give one result or a named fault, with a fixed order of coincident
failures (6.3): without them `G` would not be a function of the input. The proof covers the calls of the interface
without the additions of 2026-11-25; a width query (an ordinary answer that does not act on the qubits) and a named
stop (an end of the run with its fault) are covered only by claim 12.

**Claim 3.** Take an accepted correct compilation of a program such that, on the run input and for every list of
answers, no permitted execution reaches undefined behaviour. For every finite observation of its run (a finite stretch
of events with their records and answers, possibly with its ending), under P1 for that observation, the run's records
and answers follow one path of one of the source's permitted trees, the ending included if one is observed, up to a
resource stop of the compiled run. That this extends to a whole infinite run does not follow from the assumptions,
and a counterexample shows it: a program picks a hidden word `w` of `L` bits by an unspecified order of evaluation,
then repeats a Hadamard and a measurement on one qubit and exits when the last `L` outcomes equal `w`; on a target
with fewer than `2^L` memory configurations, a compilation that never exits is correct under A1, yet on an outcome
sequence containing every `L`-bit word its run follows no permitted path. (Nothing shows that `clang` emits such a
compilation.)

**Claim 4.** For a determinate program, two accepted correct compilations, given the same answer whenever both reach
a measurement, have the same records, answers and endings up to the first resource stop of either, under P1 for each
on every prefix and ending compared. At each successful `apply` both stand for the same source event with its `D` and
`F`; that their `G`s agree there is claim 5.

**Claim 5.** Take a determinate program and an accepted correct compilation. At each successful `apply` that
corresponds to a source event (claim 3), and each input `x` in that event's `D`, under P1, P2, P3 and P6 at that
`apply` and `x`: `G(x)` agrees with `F` on `x` or is a resource fault (`budget`, `stack_overflow`, `out_of_memory`).
So two such compilations whose `G`s at corresponding successful applies are not resource faults on `x` map every
basis configuration with that `x` alike. Nothing is claimed for `x` outside `D`. Without P2 and P3 this is open: the
natural observer (answer, call back, report the result, exit) gives no report for a function that runs forever, and
nothing guarantees that the comparison run reaches the call.

**Claim 6.** Take a determinate program and two accepted correct compilations under P4 and P6. Suppose that every
input is in the domain of every successful `apply`, and that on no history of answers (those of probability zero
included) does either compilation stop for lack of resources outside an applied function, and no `G_i(x)` is a
resource fault, for any input `x` (inputs of zero amplitude at that call included). Then the two compilations have the
same meaning: for every `n` and every list of answers, the same composed operations up to the `n`-th event, the same
output bytes and the same ending. Neither condition can be dropped. One fault-free run is not enough: after a fair
measurement, one outcome can lead to an `apply` whose body one compilation makes one step long and another two; with
budget 1 the second faults, on a history a single run may never take. Nor are the inputs a run reaches enough: apply
a function to a register that is 0; if only one compilation ends within the budget on input 1, their operations differ
on that basis configuration, though no run of the program puts amplitude on it. On a finite target P4 cannot hold
once a run can measure more often than the target can script; agreement up to the `n`-th event for `n` within the
target's reach is expected from the same proof but was not stated as a claim.

**Claim 7.** (a) For every total `G` and operands that pass 6.2, the map of 6.4 is a unitary on the named qubits, its
own inverse, and the identity elsewhere: proved. (b) For each such `G` the compute, copy and undo construction carries
it out and returns every scratch qubit to 0: proved (existence); the decision of 2026-11-25 makes it a requirement on
every implementation. (c) Under A2, the only operations of a run, on its full allocated system, that are not unitary
or an allocation are the `bq_measure` and `bq_free` calls the program made: no program loses coherence by a decision of
the compiler. Proved. It says nothing about the purity of part of the system. (d) The fault readout of 6.4, in its two
cases: proved.

**Claim 8.** (a) A successful gate, rotation or `apply` with control list `S` is a unitary that commutes with
measuring `S`, also when qubits of `S` are *in* fields: proved. (b) For unitaries that each preserve the two basis
values of a qubit `c` (phases allowed), the product of each controlled by `c` is their product controlled by `c`:
proved. (c) For an `apply`, testing the controls only on the copy of the result data, between the compute and the
undo, carries out the controlled unitary: proved. (d) The scope library. Take a finite stretch of a run between a
`bq_push` of `c` and its matching `bq_pop`, with the list last in, first out, in which every gate, rotation and
`apply` goes through the library's wrappers, only the library changes its list, every call succeeds and `c`'s register
stays allocated. The operations *without that push* are defined on the recorded events: in each wrapped operation
delete just the entry that push made from its control list, and keep every other operand and event, and at each
`apply` the function `G` the actual run determined. Then each operation in the stretch carries `c` in addition to its
controls without that push. Given the earlier outcomes and the ordinary state at its start, a stretch without
measurement makes a fixed sequence of calls whatever the quantum value of `c`; if it also has no allocation or
release, the composed unitary is the one without that push, controlled by `c`. The same holds on the other qubits if
every allocation and release in the stretch pairs within it and the operations without the push return each such
register to 0 at its release, for every input of the registers that remain. Proved (A2, A3). The comparison never
repeats a placement or an applied function's evaluation with the shortened list; it uses the `G` of the actual run,
and says nothing about a run of the program with a shorter list. The claim is false if "without that push" is read as
the C program with the push and pop deleted: with `c = 1` and `t = 0`,
`bq_push(c, 0); if (bq_scope_depth()) bq_x(t, 0); bq_pop();` sets `t = 1`, while deleting the push and pop also deletes the gate.

**Claim 9.** (a) Take a finite circuit given as a tree: it starts with no register allocated; its operations are
allocations, releases, measurements, gates and the rotations `BQ_ROT_X`, `BQ_ROT_Y`, `BQ_ROT_Z` (the width query and
the phase rotation are additions, claim 12), each with operands and controls that are representable and pass every
check of its call in 5.3 or 5.4, rotations with finite angles; so every measurement is of a slice of an allocated
register, 1 to 64 qubits wide and within the register, and a measurement of 65 qubits is not covered. Every
measurement of `b` qubits has a successor for every `t` below `2^b`, outcomes of probability zero included; and each
leaf is an exit with status 0. A control program of interface calls with constant operands, tests of the outcomes and
exits describes it. Under P5 for every path, every accepted correct compilation of it agrees with the circuit up to
the first resource stop of either side (that part may be empty), and if neither side runs out of resources on any path
up to the exit, the compilation has the circuit's meaning: proved (A1, A2; P5 takes the place of A4's comparison). A
tree that allocates a qubit, measures it and gives a successor only for 0 is not covered, although outcome 1 has
probability zero. (b) For every applied function the front end accepts, and every image and field assignment 6.2
allows, the device carries out the unitary of 6.4 for the compiled function (A2, A3): proved. (c) Take a direct device
`apply` of a compiled `g` on `k` 64-bit inputs `x`, `k` from 0 to 4, that under budget `B` returns or gives a fault
other than a resource fault, and a scalar call of 8.3 for the same compiled `g` that passes 6.2, whose setup succeeds
and, when the wrapper returns, whose extraction succeeds, whose *in* fields hold `x` (zero-extended) and whose *out*
field reads the low `b` bits of the result member, `1 <= b <= 64`. Compare the same *out* bits and the same five fault
bits, under the same test that the controls are 1. Suppose: both run on one deterministic instruction evaluator, with
the code of nested calls and the same fault codes; the compiled wrapper reaches the entry of that compiled `g` with
`x`, and every value `g` reads before writing it is at that entry the same as in the direct call (which excludes a `g`
that reads a stack pointer the wrapper moved); after `g` returns, the wrapper continues without fault through its own
return and leaves the low `b` bits of the result in the *out* field; both count one step per instruction attempted,
`w` bounds the wrapper's own steps, and `B + w <= 2^64 - 1`. Then under budget `B + w` the scalar call gives the same
low `b` result bits or the same fault code as the direct call: proved. Resource faults of the direct call are outside
it.

**Claim 10.** The only refusals on account of a primitive are those of the declaration rule (section 4): another
global with a primitive's linker name (clause 1), a header declaration that stands for another symbol or has other
properties at the end of the unit (clause 2), and a call of a primitive that is not the header's call (clause 3),
which includes a call through a `const` pointer that the compiler folds into a direct call carrying the pointer's
`const` or `pure`. A call through a pointer that stays one is never refused. At a call site the front end translates an interface call wherever it would translate a
call of an unknown external function of the same type. This is a requirement on the front end, not a consequence of A1
to A4: a translator that adds a filter refusing direct calls of `bq_measure` satisfies them just as well. Equal
acceptance at different optimisation levels is not claimed. The proof reads "name" as the C identifier; with the
linker-name clauses of section 4 as built (`docs/DESIGN.md`, "The declaration check") it is open and is to be checked
again against that check.

**Claim 11.** Grouped by cause, the known wrong results of the marker front end (`HANDOFF.md`) have no counterpart:
optimiser rewrites of a qubit-dependent condition, store or choice (a call with explicit controls or an `apply` has no
classification to go wrong; that the calls follow the source and agree across compilations is claims 3 and 4, with
their premises); exchange placement (C memory never holds qubits); a value computed more than once (one `apply` into a
register that every use reads gives one value; that two compilations prepare it alike is claim 5, with its premises);
reserved names (the declaration rule). Proved for these forms, assuming the declaration rule with linker names.

**Claim 12.** The proofs were made for the interface without the additions of 2026-11-25: `bq_width`, the phase
rotation, the two named stops, the libraries' use of `bad_argument` and `out_of_memory`, the explicit width check in
the scalar forms, the fixed layouts of 5.1 and 6.1, and the second and third clauses of the declaration rule. None has been checked
against them. The expected arguments are short: `bq_width` returns a function of the register table, so claim 1's
argument covers it; the phase rotation is a controlled unitary of the same shape as the other gates (claims 7 and 8);
a named stop is ordinary code ending in a named fault; the width check removes the layout condition from claim 9(c).
Open until checked.

## 12. What is not claimed

- Nothing about cost. Every `apply` passes its arguments through memory inside the applied function, so the gate
  emitter's handling of memory decides its price.
- Nothing across compilations for inputs outside an applied function's domain: undefined behaviour, results that
  depend on the number of an address, on a frame or return address, or on an unspecified choice, and calls left by
  `longjmp`. On the device each such input still has one outcome (claim 2).
- The resources needed differ between compilations, and so do the step counts of section 7.
- A fault code in a fault register can be cleared by the program. Reading it is a measurement, with a probability,
  and the stated probability holds only for the readouts of 6.4.
- A program with several permitted behaviours has no single source meaning; each compilation follows one on every
  finite observation (claim 3), not necessarily on a whole infinite run.
- That a given compilation is correct, that a given front end keeps to claim 10, and that the test harnesses P1 to P6
  exist, are hypotheses.
- Programs written for the marker interface do not carry over automatically.

## 13. Confirmed choices

The decisions of 2026-11-25 settled the choices the proofs left open. The maintainer confirmed the choices below
on 2026-11-27.

- **Names.** The prefix `bq_` and `BQ_`; gate codes `BQ_GATE_*` and rotation codes `BQ_ROT_*` with distinct numbers;
  directions `BQ_DIR_IN` and `BQ_DIR_OUT`; field macros `BQ_IN`, `BQ_OUT`, `BQ_IN_ALL`, `BQ_OUT_ALL`; an include guard
  that no macro of the header can collide with. Linker names equal C identifiers.
- **The named stops** (chosen on 2026-11-25, listed here for the record, not awaiting a proposal): two reserved
  primitives, `__bennett_out_of_memory` and `__bennett_bad_argument`, each translated to the device's `fault NAME`,
  and no primitive that raises an arbitrary code. The libraries report their own failed checks as `bad_argument` and
  a list that cannot grow as `out_of_memory`; a code of their own (for example for a pop without a push) would need a
  new device code.
- **One scope per program.** The scope library's state is one list for the whole program, a weak definition in the
  header (8.4), so that a routine in another translation unit sees the scope it is called under. A `static` list in
  the header would give one per translation unit.
- **Field macros check the member's size.** `BQ_IN` and `BQ_OUT` stop with `bad_argument` when the field's bytes do
  not fit the member they name (8.1). The interface is unchanged by it.
- **The placement rule** is fixed by the device and stated in `docs/DESIGN.md` when it is built. This document lets it
  read only `size` and where the heap and live stack lie at the call (6.3), so that the budget and the other operands
  cannot move the block; that is the narrowest reading, and widening it later breaks nothing written here except
  section 7's argument, which would then need the budget excluded explicitly. A natural rule: the highest multiple of
  16 at which the block ends at or below the stack pointer at the call, with `f`'s frames below it.
- **A named stop inside an applied function** gives its own code (`bad_argument` or `out_of_memory`), as any `fault
  NAME` of ordinary code does, not the class code of an output or exit.
- **How a measured budget reaches the program** (7): a definition of the build, `BQ_BUDGET_name`, from a measuring
  build whose run reports the steps used. Taken because it needs nothing from the device beyond the report; the
  alternative, a run input, needs run arguments under the C library (R6) and allows a budget computed at run time
  from a measurement made in the same run.
- **The function operand of `apply`** is settled by the decision of 2026-11-25 that each interface call is one
  device instruction: it is a run-time operand of the device's `apply` instruction like every other operand, and the
  device makes the in-region test (8) and the `bad_function` check (4) itself (5.7, 6.2, D11, D14).

## 14. What the device must provide

Every operand of an interface call is a run-time value: the handle, a slice's lowest qubit and width, a gate's kind
and qubit position, a rotation's axis and angle, and a control list given as an address and a count. Each interface
call is exactly one device instruction, and the device makes all of that call's checks in the order of section 5:
inside an applied function `quantum_in_region` (8) first (D14); outside, for a gate or rotation, `bad_gate` (23) for a
kind or axis that is not one of the codes of 5.1, then for a rotation `bad_angle` (18), then `bad_handle` (9),
`bad_range` (22), the control list (`bad_argument` 24, then `bad_handle` and `bad_range` per entry) and
`control_in_use` (10). The front end does not switch on the kind and issues no `bad_gate` itself. "Today" refers to
`crates/ir`, `crates/exec` and `docs/SYNTAX.md` on the commit that last changed a row of this table.

| # | The device provides | Needed by | Today |
|---|---|---|---|
| D1 | Handles as `i64` values: the register made by the `k`-th successful allocation of the run is `k`, from 1, never reused; a value naming no allocated register is `bad_handle` (9, widened meaning) | 3, 5.3 | yes: `i64` handles in gates, controls and `qfree` (`docs/SYNTAX.md`, "Handles as integers"); `measure` of a whole register, `apply` and calls still take `qN` registers |
| D2 | Allocation with a run-time width, returning the handle, with checks `bad_width` (20) and `out_of_handles` (21); release by handle | 5.3 | yes: `qalloc i64 %w` with both checks, `qfree` of an `i64` handle |
| D3 | The width limit of a register raised to the planned limit (`1` to `2^32 - 1`) | 5.3, 6.1 | yes: 1 to 2^32 - 1 qubits |
| D4 | The width query `qwidth`: an ordinary value from the register table, check `bad_handle` | 5.3, 8.1 | no |
| D5 | Measurement of a slice with run-time handle, lowest qubit and width (`1` to `64`), checks `bad_handle`, `bad_range` (22), result zero-extended | 5.3, 8.2 | no: `measure` takes a whole register or bits at an address (debt: `%r[LO:HI]`) |
| D6 | One gate instruction and one rotation instruction whose kind or axis (the codes of 5.1), handle, position and control list are all run-time operands, with the checks of 5.4 in order: `bad_gate` (23) for a code outside the set, for a rotation then `bad_angle` (18), then `bad_handle`, `bad_range`, the control list (D7), `control_in_use` | 5.1, 5.4 | partly: handles and positions are run-time values (`x %h[%i]`, D1); the gate kind and the axis are literals and the controls are written in the instruction (D7) |
| D7 | A control list read from memory: `n` entries of 16 bytes (handle at 0, position at 8, little-endian), checks in the order of 5.4 (`bad_argument` 24, `bad_handle`, `bad_range`, `control_in_use` = `qubit_alias` 10) | 5.4, 6.2, 8.4 | no: controls are written in the instruction, where a repeated control is refused, by the validator for a repetition in the text, otherwise `qubit_alias` at run time (`docs/SYNTAX.md`, "Handles as integers") |
| D8 | Rotations by a run-time angle, `bad_angle` (18) for a non-finite one, in the instruction of D6 | 5.4 | yes for `rx`, `ry`, `rz` and `p` with a run-time angle (`Inst::RotateBy`); the instruction of D6 is not built |
| D9 | The phase rotation `p(angle) = diag(1, e^{i angle})`, with controls | 5.4, 8.4 | yes: `p` with a literal or run-time angle, with controls |
| D10 | `apply` as one device instruction, with the function as a run-time operand (D11), an image and a field list in memory (40-byte entries of 6.1), a run-time budget, a fault register by handle, controls by D7; the checks of 6.2 in order; placement, snapshot, setup, run and extraction of 6.3; the unitary of 6.4. A fault of the applied function on some input is a value XORed into the fault register, never a stop of the run: every execution path of the handle interface keeps private faults as register values. A diagnostic mode that stops at such a fault may exist, but it is outside this meaning and must say so | 6 | no: today's `apply` passes registers as integer arguments to a function with integer parameters; and the simulator by default stops the run at the first private fault, before the XOR, unless `--machine-meaning` (or `--allow-region-faults`) is given |
| D11 | `bad_function` as `bad_indirect` (4) for a function operand outside the program's set of *apply targets*, or that is a primitive's device function. The apply targets are the functions of the program whose compiled signature is one pointer parameter and no result (6.2, check 1), read from the compiled module before pointers are lowered to 64-bit integers, primitives excluded. So `void a(void *)` and `void b(int *)` are both apply targets (compiled, both are `void (ptr)`; passing `b` is undefined at the source, check 1 †), while `void c(uint64_t)` is not, although its device signature equals that of `a`. The front end records the set in the device program; the device's `apply` instruction takes `f` as a run-time operand and makes this check itself, after the in-region test of D14, in the same step; it does not recompute the set from device signatures. The source test, type `bq_fn` exactly, stays a source-level rule | 6.2, 10 (P6) | partly: `call_indirect` checks targets by device signature, which cannot tell `a` from `c`; nothing marks the apply targets or a primitive's function |
| D12 | The fixed placement rule for the block: before any input is written, the same base for every input, a multiple of 16, outside heap and live stack, computed from `size` and where the heap and live stack lie at the call only (not from the budget or any other operand, nor from other memory); `out_of_memory` when there is none. Setup failures in a fixed order: `out_of_memory` before `stack_overflow` (6.3). Neither placement, setup nor the entry state of the applied function depends on the budget | 6.3, 7 | no |
| D13 | Every implementation of `apply` (the simulator's direct evaluation, the gate emitter, the counter, the OpenQASM writer) returns its scratch qubits to 0 | 6.4 | the emitter's compute, copy and undo does; to be checked for each device of the new `apply` |
| D14 | The device's instructions for the interface calls (the gate and rotation instructions with kind and axis as run-time operands, D6, and those of allocation, release, width, measurement and `apply`) make the in-region test as their first check: inside an applied function the instruction gives `quantum_in_region` (8), output `effect_in_region` (7), exit `exit_in_region` (13), before any other check of the call, then the call's checks in order outside one. A direct call is that one instruction, one step; a call through a pointer to a primitive's device function raises the code at the indirect call step. So with one step of the budget left the result is the class code (8 for a quantum call, 7 for output, 13 for exit), never `budget` (1), and `bq_gate` with kind 0 inside an applied function gives 8, not `bad_gate` | 5.7 | yes for today's quantum instructions, effect and exit, each one step; no: the new instructions of D2 to D10 must keep it, and an indirect call that raises 8 at the call step when its target is a primitive's device function |
| D15 | New fault codes: `bad_width` 20, `out_of_handles` 21, `bad_range` 22, `bad_gate` 23, `bad_argument` 24 (`out_of_memory` 19 comes from the runtime's work); 25 to 31 stay free | 5.2 | yes: the codes are assigned (`crates/ir/src/fault.rs`); `bad_width`, `out_of_handles` and `bad_range` are raised by the device's checks, `bad_gate` and `bad_argument` only by `fault NAME` until D6 and D7 |
| D16 | A per-`apply` step report in a measuring run (7): for each executed `apply`, its position in the run, the function, the inputs evaluated, the largest step count among inputs that finished, and how many reached the ceiling | 7 | partly: the simulator's library already records, for each executed `apply`, the callee and each evaluated input's step count and outcome (`JournalMeasurement` in `crates/exec`); `bennett run` reports only the sum of steps inside applied functions (`region steps`) |
| D17 | The gate emitter, counter and OpenQASM writer accept D1 to D10 | 6, 7 | no; the emitter's memory costs decide what an `apply` of this interface costs. A private-memory wrapper modelling the handle form costs the same as the register form on identity, an 8-bit predicate and Shor for 15 (1,552 Toffolis) |

The front end's own part, for the record (lane of the front end, not a device item): the declaration check of section
4 with linker names, on each unit before any optimiser pass so that a rename of a header declaration is refused
(clause 2), built in `bennett-cc` and `bennett-front` (`docs/DESIGN.md`, "The declaration check"); the force-included
header; one device function per primitive at a reserved address; each `bq_gate` and
`bq_rotate` translated to the one device instruction of D6, kind or axis passed as an operand (no switch on the codes
and no `bad_gate` issued by the front end); the set of apply targets, read from the compiled signatures before
pointers are lowered to integers, for D11; and the two named stops lowered to `fault NAME`.

## 15. What the runtime must provide

| # | The runtime provides | Needed by | Today |
|---|---|---|---|
| R1 | The C library's files take their declarations of the four effect primitives (`__bennett_effect`, `__bennett_exit`, `__bennett_out_of_memory`, `__bennett_bad_argument`) from the force-included header and drop their own | 4, 5.5 | no: `rt/musl/runtime.c` declares the first two, `start.c` the exit primitive, `devices.h` the output primitive (which `devices.c` takes from it) |
| R2 | The header force-included when the C library is compiled, so that the library passes the declaration rule | 4 | no |
| R3 | A request of `malloc`, `calloc`, `realloc` or `aligned_alloc` that the arena cannot hold is the fault `out_of_memory` (19) through `__bennett_out_of_memory`; a request the C library rejects from its arguments alone, before any system call (an overflowing `calloc` or `reallocarray`, a `malloc` of half the address space or more, an impossible alignment), still returns a null result with its error code, exactly as natively | 5.5, 6.3, 8.4, 10 (A4) | yes |
| R4 | Weak definitions merged into one object for the whole program when its units and the C library are linked (the scope list and fault set of 8.4 and 8.2) | 8.4 | to be checked: the header defines its libraries inline, with no library file |
| R5 | The C library's `realloc`, `printf` and `exit`, which the header's libraries call (`printf` to standard output, the console) | 8.2, 8.4 | yes (musl) |
| R6 | Run arguments that reach `main`, if a program is to receive a measured budget as a run input (7, 13); a budget defined by the build needs none | 7 | yes, with one limit: a program without the C library receives integer arguments (`bennett run --arg`); a musl-linked one receives `argc` and `argv` from its run image, which `rt/musl/link.py` fixes at link time |
| R7 | Loader choices fixed by the device: the C library's start-up makes no choice that depends on anything but the executable and the run input | 6.3 | yes: start-up is deterministic |
