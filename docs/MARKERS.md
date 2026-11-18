# Markers

The only thing a C program adds to use qubits. Declared in `front/include/bennett.h`; never defined.
`bennett-front` recognises the calls by name and signature, and refuses a `bennett_*` function that is not
one of these, is defined, has another signature, or is declared with attributes that would let LLVM delete,
merge or speculate its calls (`readonly`, `readnone`, `willreturn`, `speculatable`).

## Qubit variables

A **qubit variable** is a local scalar variable, at most 8 bytes, whose address a marker receives. There
is no allocation call: applying a gate to an ordinary `_Bool` makes it a qubit. The variable becomes one
quantum register of its width, allocated where the variable is and released when its function returns.
It starts at 0. A `_Bool` inside it (a `_Bool` variable, or one element of a `_Bool` array) is one qubit:
bit 0 of that byte.

Ordinary code may read a qubit variable. A value computed from one *depends on a qubit* and is computed
coherently: it may be stored into a qubit variable, and is otherwise refused (measure first). Writes into a
qubit variable are:

- `x = e` when x's value is known (it is fresh, was last given an ordinary value, or was just measured):
  the known value is XORed away and e is XORed in;
- `x ^= e`, with e not reading x: e is XORed in;
- `x = e(x)` on a one-bit variable, when `e(1) == e(0) ^ 1` (how LLVM writes `b ^= c` on `_Bool`s):
  `e(0)` is XORed in. LLVM's simplifier proves the identity, or, when `e` reads no ordinary value and the
  other qubit variables it reads have at most 8 bits that may be 1, the front checks it on every
  assignment of those bits (`y ^= q && !r`, whose negated `select` the simplifier does not fold);
- `x = g(x, a)` in place, where g is a bijection in x: a chain from x of `+ - ^ ~`, negation,
  multiplication by an odd constant, rotation and byte swap, whose other operands `a` do not read x
  (`x += y; x *= 3; x = ~x; x++;`). x is computed afresh into a temporary and the old value is
  uncomputed with the inverse.

Code under a condition that depends on a qubit, without gates, may write qubit variables freely when their
values before it are known, and may XOR into the others, with loops whose trip counts depend on qubits and
nested loops included: it is applied whole, with one budget (`docs/DESIGN.md`, "Code under a qubit").

Any other write (`x = x * x`, `x = 5` on an `x` in superposition) keeps x's old value: it is exchanged into a
fresh register, which is released at once (the partial trace), and the front warns with the number of qubits
kept; interference between configurations with different old values is then lost (`docs/DESIGN.md`,
"Overwrites that keep the old value"). When the function returns, a variable whose
value is known is returned to 0 first; any other is released as it is: the partial trace (`docs/DESIGN.md`,
"Releasing a register"). Where the variable is still entangled with others, they are left in a mixture and
their interference is gone; this is no fault, and the simulator reports the release (`bennett run` prints a
line), which the program cannot see.

A computation applied coherently (by `apply`, `docs/DESIGN.md`) may fault for some values of its arguments (a
division by zero, an exhausted budget). `bennett run` stops at such an `apply` and reports it by name, unless it
is run with `--allow-region-faults` or `--machine-meaning`. With either option the fault code stays
in the fault register until the function returns, where the register is released and the release
is reported.

## Memory written under a qubit

A store into memory whose value, or whether it runs, depends on a qubit keeps the value in superposition:
the bytes it writes **hold qubits** from then on (`docs/DESIGN.md`, "Memory written under a qubit"). This
holds for a global variable, the memory a pointer argument points to and a local variable, at constant
offsets, and, inside code under a qubit without gates, at positions LLVM bounds to at most 8 bytes of the
object (a loop over a small array, `memset`, `memcpy` of a length that depends on a qubit):

```c
unsigned char cells[4] = {1, 2, 3, 0};
...
if (q) cells[0] = 7;                       /* cells[0] holds 7 where q is 1, 1 elsewhere */
if (q) memset(cells, r ? 5 : 6, 4);
unsigned long long v = bennett_measure_bytes(cells, 4);   /* measured: ordinary again */
```

The front warns at the first such store, naming the bytes. Code that depends on a qubit may read them and
write them again, with the forms of qubit variables (an overwrite whose old value cannot be recovered keeps
it, with that warning). Ordinary code may not read them: it is refused, asking for the measurement, which the
program writes with `bennett_measure_bytes`; the compiler never inserts one. Bytes of a global or of memory
passed by pointer still hold qubits when the function returns; ordinary code elsewhere that reads them faults
(`qubit_memory_access`), and so does a later function's first store under a qubit into them, which reads their
old value (measure them first). A local's value is kept in qubits in a register, never in its stack bytes.
The 8-byte limit applies today. Wider held objects and a device instruction to drop bytes' qubits
unexamined are planned.

## Gates

A gate under a condition on a qubit is controlled by it: `if (a) bennett_x(&b);` is a CNOT, and
`if (a && b) bennett_x(&c);` a Toffoli. A condition that is not a conjunction of single qubits
(`if (f(p, q) == 3)`) is computed into a fresh qubit and uncomputed after the gate. Writes into qubit
variables under such a condition are controlled the same way. The condition may not read the variable the gate
acts on, nor a value read from a qubit variable that changed between the read and the gate
(`_Bool old = q; bennett_x(&q); if (old) bennett_x(&y);`): both are refused by name.

```c
void bennett_x(_Bool *qubit);    void bennett_h(_Bool *qubit);    void bennett_z(_Bool *qubit);
void bennett_s(_Bool *qubit);    void bennett_sdg(_Bool *qubit);
void bennett_t(_Bool *qubit);    void bennett_tdg(_Bool *qubit);
void bennett_rx(_Bool *qubit, double theta);
void bennett_ry(_Bool *qubit, double theta);
void bennett_rz(_Bool *qubit, double theta);
```

Apply the gate to the qubit `qubit` points to. Rotations are `exp(-i theta sigma / 2)`. Today
`theta` must be constant after optimisation. Accepting an ordinary angle computed at run time
is planned for the marker and the device language. Matrices are those of the device program
(`crates/exec/src/gates.rs`).

```c
_Bool *bennett_bit(void *variable, unsigned bit);
```

The qubit of bit `bit` (a constant after optimisation) of the qubit variable `variable` points to, for
the gates only: `bennett_h(bennett_bit(&x, 3))`. A loop over the bits works when LLVM unrolls it.

A loop with gates under such a condition runs as written, its number of passes an ordinary value:
`if (q) for (i = 0; i < n; i++) bennett_x(&y);` flips `y` n times where `q` is 1, one controlled X per pass.

## Measurement

```c
unsigned long long bennett_measure(const void *variable);
```

Measures the whole qubit variable `variable` points to, in the computational basis, and returns its value
zero-extended. Afterwards the variable holds that value (it is known), and ordinary code may use both.

```c
unsigned long long bennett_measure_bytes(const void *address, unsigned bytes);
```

Measures the `bytes` bytes at `address` (`bytes` a constant from 1 to 8), little-endian, and returns their value
zero-extended. It is the device program's `%d = measure iN [ADDR]` with `N = 8 * bytes`, nothing more: the bits
of bytes that hold qubits are measured in the computational basis, in one draw, the bits of other bytes are read
as they are, and every byte measured is ordinary memory afterwards, holding the result (`docs/DESIGN.md`, "Qubits
in memory"). On ordinary bytes it is a read. On the address of a whole qubit variable it measures the variable,
as `bennett_measure` does; on part of one it is refused. It is the measurement a program writes where ordinary
code needs a definite value from memory that holds qubits; the compiler never inserts one. Under the control of
a value that depends on a qubit it is refused, as every measurement is.

## Calls with qubit-dependent arguments

A call whose arguments depend on a qubit, to a function of the program that performs no quantum operation
(directly or through anything it calls), is **applied whole**: `r ^= f(x)` becomes one `apply` of a function
that calls `f`, with one step budget (`--region-budget`) for everything `f` does, its loops, nested loops and
calls included. `f` may write memory, allocate and free on the heap, keep static buffers and use files of the
file model. For each definite value of the arguments it runs on a private copy of memory, and all it wrote is
undone when it ends. **Only its result survives**, XORed into the variable it is stored into. This
is the language rule. When several calls must share state, put them inside one function and
apply that function. A value of the arguments for which `f` needs more than the budget gives
the fault `budget` for that value only; a console write
(`__bennett_effect`, or a musl `printf` of a line, which writes stdout out) or `exit` inside gives `effect_in_region` or
`exit_in_region` for the values that reach it. `docs/DESIGN.md`, "Applied calls", says what this means.

These marker rules describe direct simulation, including applied calls and private memory.
The counting device and the simulator's `--apply-circuit` option compile applied integer code
to gates. Qubit-dependent choices and loops with a proved pass count can use control gates
under the same budget. The compiler reports applied calls and memory operations as refusals
with their routes in [DESIGN.md](DESIGN.md#not-yet).

Natively `f`'s writes would remain. Where the front sees that code after the call reads memory the call writes
(a global variable, or memory passed to it by pointer, read directly or by a function called directly), it
warns, and still lowers the program:

```text
warning: the call of @f depends on a qubit and is applied whole: its writes are undone when it ends, but code after it reads @counter, which it writes (at p.c:9:42), at p.c:7:8
```

The value is computed where it is used, not where the call is: memory the call reads must not change in
between, and a value used twice is computed twice, each time from the same memory.

The rule is about calls, so results depend on calls reaching the front as calls. With the project's flags
(`-O0 -Xclang -disable-O0-optnone`, as `front/test.sh` compiles) every function keeps its calls. With clang
`-O2`, clang may already have inlined a small function into its caller, and its writes are then ordinary
stores with qubit-dependent addresses or values, which are refused; `noinline` keeps the call.

Under a condition that depends on a qubit (`if (q) f(n);`, `if (q) y ^= f(n);`), a call of a function that may
write memory is refused: natively its writes survive the condition, and keeping them would need the value of
the condition in ordinary memory.

A gate loop with an ordinary count can move out of qubit control when its ordinary work is proved safe,
finite without counter wrap, and free of visible effects. The counter's start and bound must be fixed
before qubit control. The moved loop takes the same passes it would take with its gates enabled. Moved memory accesses,
calls, and division or remainder without a proved nonzero divisor are refused. A guard load that
LLVM has hoisted is refused if its bytes cannot be proved ordinary. The route is to compute
the ordinary part inside one applied function for the table of control bits in [DESIGN.md](DESIGN.md#code-under-a-qubit).

A helper may read a return address in one applied evaluation. The front refuses a repeated evaluation
when the helper or a function it can call reads a return address, observes a frame address, or converts a pointer
whose address may depend on the stack to an integer, including when separate predicated
stores would repeat one source call. A refusal can name a possible indirect target, read-only
output splitting, or evaluation of a moved gate-loop condition. Keep and release the old value explicitly,
or pass the caller's identity as a separate argument to a caller-independent helper.

## Not handled yet

A named lowering refusal leaves any requested device and LLVM output files untouched.

Each is refused with `not handled yet: <what>, at <file:line:col>`: under the control of a value that
depends on a qubit, a call of a function that has gates, a measurement, a gate whose condition reads the
gate's own variable, a gate inside a loop whose number of passes depends on a qubit (write it as an ordinary
count and test the condition inside) or that decides on qubit-dependent values carried from pass to pass
(the table for constant counts and the later form for any ordinary count are described in
[DESIGN.md](DESIGN.md#code-under-a-qubit)), a write of a qubit variable
inside a loop that does more than XOR into it when its value before is not known, a store to memory in code
that also has gates, an atomic or volatile write, a call of a function that may write memory, or a volatile
read; a store under a qubit into memory whose object the front cannot name (not a global variable, a pointer
argument or a local variable), at a position it cannot bound, or into more than 8 bytes of one object; an
access that touches bytes holding qubits only in part, or at a position that is not constant outside code
applied whole; ordinary code that reads bytes holding qubits (measure them first), or a call given memory of
an object while bytes of it hold qubits, a callee that measures them included (measure them in the caller);
a second write under a qubit that would hold bytes a first one still holds, where the first does not run before
the code under a qubit on every path; a measurement of part of the bytes of a local written under a
qubit; a branch on such a value whose arms do not
rejoin before the function returns; a qubit variable passed to a
function, or wider than 8 bytes; a write into part of a qubit variable; `bennett_measure_bytes` with a number
of bytes that is not a constant from 1 to 8, or on part of a qubit variable; a value that depends on a qubit used
by ordinary code (returned, stored to ordinary memory, used as an address of memory that is not constant), or
by an access with an effect of its own (an atomic update, a
volatile or atomic read), even when its result is unused; a value read from a qubit variable used after that
variable changed; a loop that re-initialises a qubit variable whose known value differs from pass to pass; a
`setjmp` in a function with qubit variables; a call with qubit-dependent arguments whose result is not used,
to a function that may write memory (applied, it would do nothing); two such calls in one computation when one
of them may write memory (each applied call starts from the memory as it was, so the second must not see the
first's writes); such a call through a pointer, or of a function with no definition. `bennett_bit` with an index that is not constant, or its result
used other than by a gate.

## Device calls (not markers)

```c
void __bennett_effect(char byte);
_Noreturn void __bennett_exit(int status);
```

Declared in `bennett.h`, this emits one byte to the device console (`effect`). It is journalled in reversible
execution and faults with `effect_in_region` inside an applied function. It does not make its callers control
functions: a call of it with an argument that depends on a qubit is refused (it has no definition to apply), and
a call under qubit control as an ordinary call that may write memory. Definitions, other signatures, and
attributes that permit deleting or moving the call are refused by name, as for markers.

`__bennett_exit` lowers to the `exit` terminator: it ends the whole run with `status` as the entry's result,
including from a nested call or loop. The selected entry must return `i32`; another result type gives the
named fault `exit_type_mismatch`. Inside an applied function it faults with `exit_in_region`. It is journalled,
is not a marker, and has the same declaration checks as the console call, including its `noreturn` attribute.
It does not run C cleanup itself: a library's `exit` runs handlers and flushing before calling this boundary.

## C startup

Constructors and destructors run automatically around `main`, or the front's `--entry=name`. Pass the same
entry to `bennett run --entry name`. Constructors run in ascending priority order, destructors in reverse
order after a normal return; the entry's result is preserved. Ties run in the order a native build of the same
module runs them. A module that references (after the front's optimiser) any standard
`__init_array_start`/`__init_array_end` or `__fini_array_start`/`__fini_array_end` owns startup itself: the front
supplies the arrays and bounds, and the library's C startup and exit code must walk them. The bounds are defined
only after the optimiser, as a linker defines them, so a reference is lost only with dead code or an unused value,
never by folding a read of the array. The device exit call
bypasses the automatic destructor wrapper, just as `_Exit` bypasses C library cleanup.
