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
- `x = e(x)` on a one-bit variable, when LLVM proves `e(1) == e(0) ^ 1` (how it writes `b ^= c` on
  `_Bool`s): `e(0)` is XORed in;
- `x = g(x, a)` in place, where g is a bijection in x: a chain from x of `+ - ^ ~`, negation,
  multiplication by an odd constant, rotation and byte swap, whose other operands `a` do not read x
  (`x += y; x *= 3; x = ~x; x++;`). x is computed afresh into a temporary and the old value is
  uncomputed with the inverse.

Any other write would discard x's old value, and is refused. When the function returns, a variable whose
value is known is returned to 0 first; any other is released as it is, and if it is not 0 on every branch
the run ends with the fault `dirty_free` (a dirty discard is a fault in this version).

A computation applied coherently may fault on some branches (a division by zero, an exhausted region
budget). That is a region fault: `bennett run` stops at it and reports it by name, unless it is run with
`--allow-region-faults`; then the fault stays in the state, and the run ends with `dirty_free` when the
function returns.

## Gates

A gate under a branch on a qubit is controlled by it: `if (a) bennett_x(&b);` is a CNOT, and
`if (a && b) bennett_x(&c);` a Toffoli. A condition that is not a conjunction of single qubits
(`if (f(p, q) == 3)`) is computed into a fresh qubit and uncomputed after the gate. Writes under such a
branch are controlled the same way. The condition may not read the variable the gate acts on.

```c
void bennett_x(_Bool *qubit);    void bennett_h(_Bool *qubit);    void bennett_z(_Bool *qubit);
void bennett_s(_Bool *qubit);    void bennett_sdg(_Bool *qubit);
void bennett_t(_Bool *qubit);    void bennett_tdg(_Bool *qubit);
void bennett_rx(_Bool *qubit, double theta);
void bennett_ry(_Bool *qubit, double theta);
void bennett_rz(_Bool *qubit, double theta);
```

Apply the gate to the qubit `qubit` points to. Rotations are `exp(-i theta sigma / 2)`; `theta` must be a
constant (after optimisation). Matrices are those of the device program (`crates/exec/src/gates.rs`).

```c
_Bool *bennett_bit(void *variable, unsigned bit);
```

The qubit of bit `bit` (a constant after optimisation) of the qubit variable `variable` points to, for
the gates only: `bennett_h(bennett_bit(&x, 3))`. A loop over the bits works when LLVM unrolls it.

## Measurement

```c
unsigned long long bennett_measure(const void *variable);
```

Measures the whole qubit variable `variable` points to, in the computational basis, and returns its value
zero-extended. Afterwards the variable holds that value (it is known), and ordinary code may use both.

## Not handled yet

Each is refused with `not handled yet: <what>, at <file:line:col>`: under the control of a value that
depends on a qubit, a call of a function that has gates, a measurement, a gate whose condition reads the
gate's own variable, a quantum operation inside a loop (fully unrolled loops work), a write to memory, a
call of a function that may write memory, or a volatile read; a branch on such a value whose arms do not
rejoin before the function returns; an update of a qubit variable that is not a bijection listed above
(`x = x * x`, `x += x`): that needs its history kept, which is a later step; a qubit variable passed to a
function, or wider than 8 bytes; a write into part of a qubit variable; a value that depends on a qubit
used by ordinary code (returned, stored to ordinary memory, passed to a function that writes memory, used
as an address of memory that is not constant); a value read from a qubit variable used after that
variable changed; a loop that re-initialises a qubit variable whose known value differs from pass to
pass. `bennett_bit` with an index that is not constant, or its result used other than by a gate.
