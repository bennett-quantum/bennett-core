# Text form

One statement per line; `;` starts a comment. The single reader is `bennett_ir::read`; the writer is
`bennett_ir::write`, and for any module read from text `read(write(m)) == m`.

## Grammar sketch

```
module     := { "memory" INT | "data" INT "=" item {"," item} | function }
item       := STRING | iN INT | iN @func                 ; bytes, integer, function address
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
            | %d = faddr @f
            | effect operand                             ; one i8 to the console
            | %d = qalloc N
            | qfree %r
            | GATE qubit [ctrl qubit {, qubit}]          ; x h z s sdg t tdg
            | (rx | ry | rz)(ANGLE) qubit [ctrl ...]     ; radians
            | %d = measure %r
            | apply @f(typed, ...) -> %out, %fault budget operand
terminator := br label[(typed, ...)]
            | br operand, target, target
            | switch iN operand, INT: target, ..., default: target
            | ret [operand]
            | fault NAME                                 ; budget, unreachable, ... (bennett_ir::Fault)
```

Integers are decimal or `0x` hex and may be negative (two's complement at the type's width). Strings
accept `\n \t \r \0 \\ \" \xHH`. Names may be used before they are defined.

## Memory

`memory N` gives N bytes, all zero except the `data` segments. The stack grows down from the top and
may not reach below the highest data byte: each call reserves 8 bytes per slot of the callee's frame,
and `alloca` reserves its size (8-byte aligned). Running out is the fault `stack_overflow`.

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
