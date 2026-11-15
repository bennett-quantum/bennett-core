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
type       := iN | qN                                    ; iN: N from 1 to 128; qN: a register handle, N from 1 to 64

operand    := %name | INT          ; an INT takes its type from the instruction
typed      := %name | iN INT       ; in argument lists, a constant names its type
qubit      := %name ["[" INT "]"]  ; bare %q means %q[0]

statement  := %d = BINOP iN operand, operand             ; add sub mul udiv sdiv urem srem and or xor shl lshr ashr
            | %d = icmp PRED iN operand, operand         ; eq ne ult ule ugt uge slt sle sgt sge
            | %d = select iN operand, operand, operand
            | %d = (zext | sext | trunc) iN operand to iM
            | %d = load iN operand                       ; address is i64
            | store iN operand, operand
            | %d = alloca operand
            | %d = stacksave                             ; the stack pointer, as an i64
            | stackrestore operand                       ; back to a saved stack pointer; else fault bad_stack_restore
            | [%d =] call @f(typed, ...)
            | [%d =] call_indirect fn(type, ...) [-> type] operand(typed, ...)
            | %d = setjmp operand                        ; i32 0; writes a jump record at the i64 address
            | %d = faddr @f
            | effect operand                             ; one i8 to the console
            | %d = qalloc N
            | qfree %r
            | GATE qubit [ctrl qubit {, qubit}]          ; x h z s sdg t tdg
            | (rx | ry | rz)(ANGLE) qubit [ctrl ...]     ; radians
            | %d = measure %r
            | apply @f(typed, ...) -> %out, %fault budget operand
            | %d = mark                                  ; an i64 handle for rewind
            | rewind %h [keep (typed -> %r {, typed -> %r})]  ; XOR each value into its register, un-run since %h
terminator := br label[(typed, ...)]
            | br operand, target, target
            | switch iN operand, INT: target, ..., default: target
            | ret [operand]
            | exit typed                                 ; finish the whole run, even from a nested call
            | longjmp operand, operand                   ; jump record address (i64), i32 value
            | fault NAME                                 ; budget, unreachable, ... (bennett_ir::Fault)
```

Integers are decimal or `0x` hex and may be negative (two's complement at the type's width). Strings
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

## History: `mark` and `rewind`

`%h = mark` defines an `i64` handle naming the current point of the history. `rewind %h` un-runs every
step since that point and the program continues after the rewind; `rewind %h keep (%v -> %r, i2 1 -> %s)`
first XORs each value into its register, in order. `%h` must be the result of a `mark` in the same
function (the validator's `NotAMark`), not a value derived from one or passed through a block parameter.
Each kept value is an integer exactly as wide as its register (a typed constant names its width); keeping
a register's value is not accepted. `keep` with an empty list is not accepted: write `rewind %h`. The
meaning, and when a rewind faults (`bad_rewind`), is in `docs/DESIGN.md`, "History: `mark` and `rewind`".

## Whole-run exit

`exit %status` or `exit i32 5` is a terminator. Its operand must be an integer; constants name their width
because the enclosing function need not return that type. When executed, its type must match the selected
entry function's result type exactly, otherwise the named fault is `exit_type_mismatch`. A void entry cannot
use `exit`. A successful exit has precisely the entry-return outcome, keeps output and memory, and is
journalled for reversal. Inside `apply` it always faults with `exit_in_region`, before checking the entry type.

## Non-local jumps

`%r = setjmp %buf` has an `i32` result and an `i64` address; `longjmp %buf, %v` is a terminator with an `i64`
address and an `i32` value. `setjmp` writes 32 bytes at `%buf`, four little-endian 64-bit words: the frame's
depth from 0 (low 32 bits) and function index (high 32 bits); the `setjmp`'s block index (low) and instruction
index in the block (high); the frame's entry stack pointer; the stack pointer when `setjmp` ran. A record or
buffer outside memory is `out_of_bounds`; a record `longjmp` cannot match to a live frame and `setjmp` is
`bad_indirect` (`docs/DESIGN.md`, "Non-local jumps"). The value is passed as it is: C's rule that 0 becomes 1
is the front's.
