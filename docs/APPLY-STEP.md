# One definite run of `apply`

## State and entry

An argument to `apply @f(args) -> out, fault budget B` supplies an ordinary integer or the definite bits of a
register, a group of qubits numbered from its low bit. [region.rs: input, apply] Validation requires exact integer
parameter widths, an integer result matching `out` and a `fault` register at least five qubits wide. It rejects
the same slot as both targets or as a target and an argument. [validate/function.rs: apply] [fault.rs: CODE_WIDTH]

After the enclosing budget check, resolve `out`, `fault`, then the arguments in order. A stale handle, a stored
identifier of a dead register, gives the enclosing run `dangling_register`. If the resolved targets coincide or
either is an argument register, the enclosing run faults `qubit_alias`. Arguments may share a register.
[executor.rs: step] [region.rs: region_operands, input] [quantum.rs: resolve]

Start with the caller's frames, stack pointer, stack floor, and private flat byte memory. The circuit receives the
caller’s memory image and the addresses of bytes that hold qubits. This set stays fixed because no quantum
instruction can act inside. Stores only change private bytes. [region.rs: evaluate] [backward.rs: unwind]
[forward.rs: execute_inst, store] [cells.rs: check_no_cells] The stack pointer is the address from which storage
is reserved downwards. The floor is the highest static data end address or zero. [executor.rs: new] [module.rs: static_end]

A frame holds its function, program point (block and next instruction or terminator), entry stack pointer
(before reservation), and a window of slots, the storage for named values. [executor.rs: Frame, Executor, slot]
Count one slot per named result and per function or block parameter, including unused ones. Integer slots hold
1 to 128 bits. Register slots hold 64-bit handles to 1 to 64 qubits, so an applied result can be at most 64 bits
while its intermediates may be wider. [reader/mod.rs: function, params, label] [reader/statement.rs: statement]
[module.rs: slot_count] [types.rs: Width::new, MAX_QUBITS] [executor.rs: handle] [validate/function.rs: apply]

Read the ordinary operand `B` and start with zero steps. Reserve eight bytes per slot, regardless of width,
so a zero-slot frame reserves zero bytes. Checked multiplication overflow, subtraction underflow or going below
the floor gives `stack_overflow` before any test of `B`. Entry costs zero steps and sets block and instruction to
zero. Every slot of a new frame is zero until written. Entry writes the arguments into parameter slots.
[region.rs: apply, evaluate] [executor.rs: SLOT_BYTES] [forward.rs: frame_fits, push_frame]

## Steps

Status is running, returned with a value, or faulted with a code. Finished status stays fixed. While running,
test steps taken ≥ `B` before each attempt and give `budget` if true. Each success adds one step, including a
jump, call or return. Faults cost zero, so attempt number `B` can return or give its own fault.
[executor.rs: step, budget_reached] [forward.rs: ret]

- **Values.** Operands read constants or current slots. Value instructions write a destination and advance.
  [executor.rs: read] [forward.rs: produce] Binary operations read both operands and retain the low bits at the
  declared width. Signed operands are two's complement. Signed division truncates toward zero and wraps on
  overflow. Signed remainder is zero or has the dividend's sign, with magnitude below the divisor's.
  Division or remainder by zero gives `divide_by_zero`. Shifts by at least the width give zero.
  [arith.rs: binary, Machine::binary, Machine::signed] `icmp` writes 0 or 1. `slt sle sgt sge` compare in signed
  order, `ult ule ugt uge` in unsigned order, and `eq ne` test bit equality and inequality. `zext sext trunc`
  zero-extend, sign-extend and keep low bits respectively. [arith.rs: compare_at, cast] `select` reads the
  condition and the chosen operand (true for 1). Earlier producers can already have faulted. `faddr` writes
  the function index plus one. [forward.rs: execute_inst] [executor.rs: run] [module.rs: FuncId::address]
- **Jumps.** `br` takes its target, conditional `br` takes the true target for 1, and `switch` takes the
  matching case or default. Read every incoming operand before writing any target block parameter, then
  move to the target's start. [forward.rs: execute_terminator, jump]
- **Calls.** A direct call reads its arguments and reserves and enters a frame as above. An indirect call first
  resolves the function address and verifies exact parameter and result types, or faults `bad_indirect`.
  The caller stays at the call site. Return reads its operand if present. A nested return pops its frame,
  restores its entry pointer, writes the call's destination if present, and advances the caller. Return from
  the applied entry sets the returned status and leaves its frame for cleanup.
  [forward.rs: call, push_frame, execute_inst, execute_terminator, ret] [module.rs: function_at, has_signature]
- **Stack.** `alloca` subtracts the size and rounds down to a multiple of eight. It faults `stack_overflow` on
  underflow or below the floor. Otherwise it sets the pointer and writes it to the destination. `stacksave`
  writes the current pointer to its destination. `stackrestore` sets the pointer to any operand between the
  current pointer and the entry pointer minus slot bytes, inclusive, or faults `bad_stack_restore`.
  An unsaved or unaligned pointer in range works. None changes bytes. [forward.rs: execute_inst, alloca, stack_restore]
- **Memory.** Load and store read a 64-bit address. Store also reads its value. Check address plus width rounded
  up to bytes for overflow or exceeding memory length (`out_of_bounds`), then check the full range for
  bytes holding qubits (`qubit_memory_access`) regardless of their values. Load writes the little-endian value
  masked to width. Store replaces the whole range with the value's low bytes, least significant first.
  Both advance. [forward.rs: execute_inst, memory_range, load, store] [cells.rs: check_no_cells]
- **Non-local jumps.** Both check bounds and qubit occupancy in that order for the 32-byte buffer. `setjmp`
  writes four little-endian 64-bit words (frame depth and function index, block and instruction index, entry
  pointer, current pointer). Indices start at zero and the first member of each pair takes the low 32 bits.
  It writes result zero and advances. [forward.rs: jump_record_range, jump_record, setjmp] `longjmp` reads the
  record and checks in order for a live depth within this run, matching function and entry pointer, a
  `setjmp` at that point, and a pointer within the target's inclusive allocation range. Its lower limit is
  the next frame's entry pointer, or the current pointer for the top frame. Its upper limit is the target
  entry pointer minus slot bytes. Failure gives `bad_longjmp`. Success drops higher frames, restores the
  recorded pointer, writes the supplied value unchanged to the `setjmp` result and resumes just after it, even if it never ran. It preserves the other slots and memory. [forward.rs: longjmp]
- **Return addresses.** `retaddr N` writes the return address of the frame `N` levels down, including suspended
  caller frames. The high 32 bits give the caller function address, the low 32 bits the position just after
  the call, counting instructions and terminators from zero in block order. The bottom frame gives
  `0xffff_ffff_0000_0000` and beyond it gives zero. [forward.rs: execute_inst, return_address] [module.rs: code_address_after, code_address]
- **Stops.** `fault NAME` gives any code in the table. Console `effect`, `exit` and any quantum instruction
  including nested `apply` fault before acting with `effect_in_region`, `exit_in_region` and `quantum_in_region`
  respectively. [forward.rs: execute_inst, execute_terminator] [reader/statement.rs: terminator]

## Fault codes

Codes 11 and 15 are unused. [fault.rs: faults, Fault::code]

| Code | Name | Code | Name |
| ---: | --- | ---: | --- |
| 1 | `budget` | 9 | `dangling_register` |
| 2 | `divide_by_zero` | 10 | `qubit_alias` |
| 3 | `out_of_bounds` | 12 | `bad_stack_restore` |
| 4 | `bad_indirect` | 13 | `exit_in_region` |
| 5 | `unreachable` | 14 | `exit_type_mismatch` |
| 6 | `stack_overflow` | 16 | `bad_longjmp` |
| 7 | `effect_in_region` | 17 | `qubit_memory_access` |
| 8 | `quantum_in_region` | | |

## Finish

Save the outcome and undo the private run. XOR flips target bits whose source bits are one. A return XORs its
low 64 bits into `out` alone, without another width mask. A fault XORs its code into `fault` alone. A run still
running at the boundary first faults `budget`. Other registers and each amplitude (the complex coefficient
of a definite register assignment) are preserved. A completed `apply` costs the enclosing run one step.
[region.rs: apply, evaluate, restore] [backward.rs: unwind] [forward.rs: execute_inst] [executor.rs: step]

The simulator normally stops the enclosing run before either XOR if any definite register assignment faults.
`--machine-meaning` and `--allow-region-faults` each disable that diagnostic and keep the fault statistics.
[region.rs: apply, count_region_faults] [cli/run.rs: run] [executor.rs: new]
