# Text form

One statement per line; `;` starts a comment. The single reader is `bennett_ir::read`; the writer is
`bennett_ir::write`, and for any module read from text `read(write(m)) == m`.

## Grammar sketch

```
module     := { "memory" INT | "data" INT "=" item {"," item} | function }
item       := STRING | iN INT | iN @func | zero COUNT    ; bytes, integer, function address, zero bytes
function   := "func" @name "(" [param {"," param}] ")" ["->" type] "{" block {block} "}"
param      := type %name
block      := label ":" {statement} terminator           ; a line ending in ":" is a label
label      := name ["(" param {"," param} ")"]           ; block parameters replace phi nodes
type       := iN | qN                                    ; iN: N from 1 to 128; qN: a register handle, N from 1 to 2^32 - 1

operand    := %name | INT          ; an INT takes its type from the instruction
typed      := %name | iN INT       ; in argument lists, a constant names its type
qubit      := %name ["[" INT "]"]  ; bare %q means %q[0]; the index is below the register's width
            | "[" operand [":" INT] "]"        ; a bit of memory: bit INT of the integer at the i64 address

statement  := %d = BINOP iN operand, operand             ; add sub mul udiv sdiv urem srem and or xor shl lshr ashr
            | %d = icmp PRED iN operand, operand         ; eq ne ult ule ugt uge slt sle sgt sge
            | %d = select iN operand, operand, operand
            | %d = (zext | sext | trunc) iN operand to iM
            | %d = load iN operand                       ; address is i64
            | store iN operand, operand
            | %d = alloca operand
            | %d = stacksave                             ; the stack pointer, as an i64
            | stackrestore operand                       ; back to a saved stack pointer; else fault bad_stack_restore
            | [%d =] call @f(typed, ...)                  ; a qN argument passes the handle to a qN parameter
            | [%d =] call_indirect fn(type, ...) [-> type] operand(typed, ...)
            | %d = setjmp operand                        ; i32 0; writes a jump record at the i64 address
            | %d = retaddr N                             ; i64 code address the frame N levels down returns to
            | %d = faddr @f
            | effect operand                             ; one i8 to the console
            | %d = qalloc N                              ; N from 1 to 2^32 - 1
            | qfree %r                                   ; the partial trace: %r dropped unexamined
            | GATE qubit [ctrl qubit {, qubit}]          ; x h z s sdg t tdg
            | (rx | ry | rz)(ANGLE) qubit [ctrl ...]     ; radians, any finite angle
            | (rx | ry | rz)(%a) qubit [ctrl ...]        ; the angle computed at run time: an i64, binary64 bits
            | %d = measure %r                            ; every qubit of the register: an iN, N at most 128
            | %d = measure iN "[" operand "]"            ; N bits from bit 0 of the integer at the i64 address
            | apply @f(typed, ...) -> %out, %fault budget operand  ; %fault: a register of 5 qubits or more
                                                                   ; meaning: docs/DESIGN.md, "apply"
terminator := br label[(typed, ...)]
            | br operand, target, target
            | switch iN operand, INT: target, ..., default: target
            | ret [operand]
            | exit typed                                 ; finish the whole run, even from a nested call
            | longjmp operand, operand                   ; jump record address (i64), i32 value
            | fault NAME                                 ; budget, unreachable, ... (bennett_ir::Fault; 5-bit codes)
```

Integers are decimal or `0x` hex and may be negative (two's complement at the type's width). An `ANGLE` is
a decimal literal that must be finite as an `f64`. A literal that overflows (`1e999`) is a read
error. In the other form the angle is the value of an `i64` slot, `%a`, read when the gate runs as the bits
of an IEEE 754 binary64 (as C's `double` is held in memory): an angle computed by the program. The validator
requires an `i64`; an angle that is not finite (an infinity or a NaN) is the fault `bad_angle` (18) when the
gate runs, before anything changes. Only a name is accepted there: `rz(1)` is the literal 1.0. The writer
prints each form as it was read, so `read(write(m)) == m`. Strings
accept `\n \t \r \0 \\ \" \xHH`. Names may be used before they are defined.

## Memory

`memory N` gives N bytes, all zero except the `data` segments. The stack grows down from the top and
may not reach below the highest data byte (`Module::static_end`, the largest `address + length` over all
segments, `zero` items included): each call reserves 8 bytes per slot of the callee's frame,
and `alloca` reserves its size (8-byte aligned). Running out is the fault `stack_overflow`.

## Data items

A `data ADDRESS = item, ...` line places its items one after another from ADDRESS. ADDRESS is an
unsigned integer (decimal or `0x` hex) from 0 to 2^64 - 1.

| Item | Bytes |
|---|---|
| `"..."` | the string's bytes, escapes decoded; `""` is no bytes |
| `iN INT` | the integer, little-endian, in N/8 bytes rounded up (`i1` takes one byte) |
| `iN @f` | the address of `@f`, little-endian in the same number of bytes |
| `zero COUNT` | COUNT bytes of value zero |

`zero COUNT`, precisely:

- `zero` is the lowercase word, recognised only where an item may start; it is not a type and not a
  reserved name elsewhere. COUNT follows it on the same line.
- COUNT is an unsigned integer from 0 to 2^64 - 1 = 18446744073709551615: decimal digits, or `0x`
  (lowercase x) followed by hex digits of either case; leading zeros are allowed. A minus sign (`-1`, also `-0`),
  a fraction, an exponent or a value above 2^64 - 1 is the read error `BadNumber`; a plus sign is not a character
  of the text form anywhere, so `+0` is `BadCharacter`. A missing COUNT, one
  that is not a number, or anything after it but `,` or the end of the line is `Unexpected`; so are `0X10`
  and a bare `0x`, which read as the number `0` followed by a word. `zero16`, without the space, is one word and
  reads as a bad type (`BadType`).
- `zero 0` is allowed and places no bytes, like `""`.
- A `zero` extent is static data like any other item: its bytes count towards the segment's length, the
  bounds and overlap checks, and `Module::static_end`, so the stack never uses them. Memory written as
  `zero N` and memory written as N `\0` bytes in a string are the same initial image with the same stack
  floor.
- The writer prints `zero` followed by COUNT in decimal and never merges or splits items, so reading and
  writing reproduce the items exactly.

The validator checks every segment: its items' lengths must add up to at most 2^64 - 1 bytes
(`DataTooLong`), `address + length` must not exceed the memory size (`DataOutOfBounds`, also when the
sum overflows), and no two segments may share a byte (`DataOverlap`; a segment of no bytes overlaps
nothing).

`bennett-front` writes every run of 16 or more zero bytes in an initializer as `zero N` and keeps shorter
runs inside the surrounding string (the reason is at `minZeroRun` in `front/src/Emit.cpp`).

## Annotated example

```
memory 64                                   ; 64 bytes; no data, so all of it is stack

func @main(i64 %n) -> i64 {                 ; one i64 parameter, one i64 result
entry:                                      ; the entry block takes no parameters
    %empty = icmp eq i64 %n, 0              ; the constant 0 is an i64, from the instruction
    br %empty, done(i64 0), loop(i64 0, i64 0)
loop(i64 %i, i64 %acc):                     ; block parameters, set by every branch here
    %acc.next = add i64 %acc, %i
    %i.next = add i64 %i, 1
    %again = icmp ult i64 %i.next, %n
    br %again, loop(%i.next, %acc.next), done(%acc.next)
done(i64 %sum):
    ret %sum
}
```

## Qubits in memory

A gate's qubit is a register qubit, `%q[3]`, or a bit of memory, `[%p:3]`: bit `3 % 8` of the byte at
`%p + 3 / 8`, so `[%p:BIT]` is bit BIT of the little-endian integer at `%p`, as `load` and `store` see it.
The address is an `i64` value or a constant (`[64:0]`); the bit is a decimal from 0 to 127; `[%p]` is
bit 0. One gate may mix them: `x [%p:1] ctrl %q[0], [16]`. A gate whose qubits are all in registers is
`bennett_ir::Inst::Gate`, as before; one with a qubit in memory is `Inst::GateAt`, and the writer prints
both in the same form, so reading what it wrote gives the same instruction. The validator reports a bit
from 128 (`MemoryBitOutOfRange`), the same address operand and bit twice in one gate
(`RepeatedMemoryQubit`; the same bit through different operands is the run-time fault `qubit_alias`),
and an address that is not an `i64`.

Ordinary code does not touch a byte that holds qubits: a `load`, `store`, `setjmp` or `longjmp` touching
one faults `qubit_memory_access`, inside an applied function too (`docs/DESIGN.md`, "Qubits in memory").

Measuring all eight bits of a byte that holds qubits makes it ordinary memory again. A planned
instruction will drop the qubits in addressed bytes unexamined by partial trace, as `qfree`
does for registers.

## Registers are not integers

A register `%q` of type `qN` passed to a parameter of type `qN` passes the handle. Nowhere does a register stand
for an integer in ordinary code: a `call`, `call_indirect` or branch passing one to an `iN` parameter is a type
mismatch. Only `apply` reads its argument registers, for each definite value of them. `out` and `fault` may not
be among an `apply`'s arguments, nor be one register. A call can return a handle to its caller. Returning a register from the
selected entry ends the run successfully with no scalar host result. The CLI reports its declared width.

A register has from 1 to 2^32 - 1 qubits, as wide as a program needs: the simulator stores one wider than
64 qubits in several columns, and a register the host's memory cannot hold stops the run with the
simulator's `QubitStorage`, not a fault of the program; a run on an external device stores none. Two forms
read a whole register as an integer, so there its width is at most 128, the widest integer:

- `%d = measure %r` gives an `iN` of the register's width. A wider register is the validator's
  `MeasureTooWide`. To measure some of its qubits, copy at most 128 of them into a fresh register with
  controlled X gates (`x %c[i] ctrl %r[j]`), measure that (the same as measuring those qubits), undo the
  copy and `qfree` the copy.
- An `apply` argument register has the width of its `iN` parameter and `out` the width of the result, so a
  wider one is `RegionOperandTooWide`. For an argument, copy the qubits into a fresh register as above,
  apply to it, undo the copy and `qfree` it. For the output, exchange the qubits with a fresh register (a
  swap: three controlled X per qubit), apply into it, exchange them back and `qfree` it, which is zero
  again: `apply` XORs into the qubits themselves, which a copy would not. The `fault` register has at
  least 5 qubits and may be of any width: the code goes into its low 5 bits.

Handles of any width pass through calls, block parameters and `ret` as before. To apply a function to
bits held at a memory address today, copy the bits into a register with controlled X gates, use
`apply`, then undo the copy. An argument form for bits at an address will expand into those steps
once C functions can take qubits through pointers.

Writes in an applied function are undone. Place calls that must share state inside one applied
function so they see each other's writes on its private copy of memory.

The gate compiler for `apply` covers a subset of integer instructions. It can use control
gates for qubit-dependent choices and loops with a proved pass count, or dispatch instructions
with a single clock for the rest of the apply. A `switch` needs a proved definite selector.
The counter uses the compiler automatically and the simulator accepts `--apply-circuit auto`.
See "Gate emitter" in [DESIGN.md](DESIGN.md#apply).

## Whole-run exit

`exit %status` or `exit i32 5` is a terminator. Its operand must be an integer; constants name their width
because the enclosing function need not return that type. When executed, its type must match the selected
entry function's result type exactly, otherwise the named fault is `exit_type_mismatch`. A void entry cannot
use `exit`. A successful exit has precisely the entry-return outcome, keeps output and memory, and is
journalled for reversal. Inside an applied function it always faults with `exit_in_region`, before checking
the entry type.

## Return addresses

`%r = retaddr N` has an `i64` result; `N` is a decimal constant up to 2^32 - 1. It gives the code address the
frame `N` levels below the current one returns to: `(f + 1) << 32 | p`, where `f` is the index of the caller's
function (so `f + 1` is its address) and `p` the position after the call in that function, counting from 0
every instruction and every terminator of its blocks in order (so `p` is at least 1). The outermost frame of the
run gives `0xFFFF_FFFF_0000_0000`, a level beyond it 0 (`docs/DESIGN.md`, "Return addresses").

## Non-local jumps

`%r = setjmp %buf` has an `i32` result and an `i64` address; `longjmp %buf, %v` is a terminator with an `i64`
address and an `i32` value. `setjmp` writes 32 bytes at `%buf`, four little-endian 64-bit words: the frame's
depth from 0 (low 32 bits) and function index (high 32 bits); the `setjmp`'s block index (low) and instruction
index in the block (high); the frame's entry stack pointer; the stack pointer when `setjmp` ran. A record or
buffer outside memory is `out_of_bounds`; a record `longjmp` cannot match to a live frame and `setjmp` is
`bad_longjmp` (`docs/DESIGN.md`, "Non-local jumps"). The value is passed as it is: C's rule that 0 becomes 1
is the front's.
