//! Test-only, independent compute/copy/uncompute on explicit basis configurations.
//!
//! Machine words encode status, result, fault, scheduled tick count, frame depth, the stack pointer,
//! frames (function, block, instruction, entry stack pointer and value slots), and byte memory. Each
//! tick has a separate record of the same size. For any total ordinary update f, the completed step
//! is (c,j) -> (j XOR f(c),c), with inverse (c,j) -> (j,c XOR f(j)), even on nonblank records.
//!
//! The ordinary update is written from `docs/APPLY-STEP.md` and `docs/SYNTAX.md` alone. No executor
//! arithmetic, stepping, evaluation or reversal code is used or was read. Where the text left a
//! choice open, the comment at that rule says what was chosen and why.

use std::collections::BTreeMap;

use bennett_ir::{BinOp, CastOp, Fault, FuncId, Inst, Module, Operand, Predicate, Target, Terminator, Width};
use num_complex::Complex;

pub mod swap_in;

const STATUS: usize = 0; // 0 running, 1 returned, 2 faulted
const RESULT: usize = 1;
const FAULT: usize = 2;
const TICKS: usize = 3;
const DEPTH: usize = 4;
const STACK: usize = 5;
const HEADER: usize = 6;

// Frame words: function, block, next instruction (the terminator when equal to the block's
// instruction count), the stack pointer at entry before the frame's reservation, then the slots.
const FUNCTION: usize = 0;
const BLOCK: usize = 1;
const POSITION: usize = 2;
const ENTRY_STACK: usize = 3;
const FRAME_HEADER: usize = 4;

/// APPLY-STEP, "State and entry": eight bytes per slot, regardless of width.
const SLOT_BYTES: u64 = 8;

/// The widest value one machine word of this reference holds. Wider instructions are declined.
const WORD_BITS: u32 = u64::BITS;

pub type Machine = Vec<u64>;

// The fixture chooses a common denominator for these exact complex coefficients.
pub type Vector<B> = BTreeMap<B, Complex<i64>>;

type Outcomes<const N: usize> = BTreeMap<[u64; N], (u64, u64)>;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Basis<B> {
    registers: B,
    machine: Machine,
    records: Vec<Machine>,
}

/// How one definite run of the applied function ends, and after how many attempted steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finish {
    /// The `(result, fault code)` pair XORed into the targets.
    pub output: (u64, u64),
    /// The number of ticks after which the run was no longer running; `None` if it was still running
    /// at the bound the caller gave. A run that finishes after `t` ticks has the same output for every
    /// budget of at least `t`, and `budget` for every smaller one.
    pub ticks: Option<usize>,
}

pub struct Reference<'m> {
    module: &'m Module,
    entry: FuncId,
    frame_size: usize,
    /// The deepest call chain from the entry, if no function is reachable from itself.
    chain: Option<usize>,
    caller_stack: u64,
    caller_memory: Option<&'m [u8]>,
    held_bytes: &'m [u64],
    parameter_slots: Vec<usize>,
}

/// The stack pointer of a function the run entered first, with nothing reserved below it but its
/// own frame: memory grows down from its top (`docs/SYNTAX.md`, "Memory").
pub fn entry_stack_pointer(module: &Module, function: &str) -> u64 {
    let caller = module.function_named(function).expect("caller");
    module.memory_size - SLOT_BYTES * module.function(caller).slot_count() as u64
}

fn mask(width: Width) -> Option<u64> {
    (width.bits() <= WORD_BITS).then(|| u64::MAX >> (WORD_BITS - width.bits()))
}

/// Two's complement at `width`, as a signed 64-bit value.
fn signed(value: u64, width: Width) -> i64 {
    let unused = WORD_BITS - width.bits();
    ((value << unused) as i64) >> unused
}

impl<'m> Reference<'m> {
    /// The reference for `apply @name` executed by `caller`, a function entered at the top of memory.
    pub fn new(module: &'m Module, name: &str, caller: &str) -> Self {
        Self::at(module, name, entry_stack_pointer(module, caller))
    }

    /// The reference for `apply @name` executed when the stack pointer is `caller_stack`.
    pub fn at(module: &'m Module, name: &str, caller_stack: u64) -> Self {
        let frame_size = FRAME_HEADER + module.functions.iter().map(|f| f.slot_count()).max().expect("functions");
        let entry = module.function_named(name).expect("applied function");
        let parameter_slots = module.function(entry).params.iter().map(|param| param.slot.index()).collect();
        let chain = call_chain(module, entry);
        Self { module, entry, frame_size, chain, caller_stack, caller_memory: None, held_bytes: &[], parameter_slots }
    }

    /// Starts with the fixture's live caller memory. Held addresses are fixed during a private run.
    pub fn with_memory(mut self, bytes: &'m [u8], held_bytes: &'m [u64]) -> Self {
        assert_eq!(bytes.len() as u64, self.module.memory_size);
        assert!(held_bytes.iter().all(|&address| address < self.module.memory_size));
        self.caller_memory = Some(bytes);
        self.held_bytes = held_bytes;
        self
    }

    /// The number of frames the encoding holds for a run of at most `budget` steps. Each call costs a
    /// step, so no run can be deeper than `budget` calls; a module without recursion bounds it sooner.
    /// Either way the size is fixed before the `apply` starts, the same for every argument value.
    fn frames(&self, budget: usize) -> usize {
        1 + self.chain.map_or(budget, |chain| chain.min(budget))
    }

    fn memory_start(&self, frames: usize) -> usize {
        HEADER + frames * self.frame_size
    }

    /// The machine before the arguments are copied in, with room for a run of `budget` steps.
    pub fn initial_machine(&self, budget: usize) -> Machine {
        let frames = self.frames(budget);
        let memory_start = self.memory_start(frames);
        let mut initial = vec![0; memory_start + self.module.memory_size as usize];
        if let Some(bytes) = self.caller_memory {
            for (word, &byte) in initial[memory_start..].iter_mut().zip(bytes) {
                *word = u64::from(byte);
            }
        } else {
            for segment in &self.module.data {
                for (i, byte) in segment.encode().into_iter().enumerate() {
                    initial[memory_start + segment.address as usize + i] = u64::from(byte);
                }
            }
        }
        initial[HEADER + FUNCTION] = u64::from(self.entry.0);
        initial[HEADER + ENTRY_STACK] = self.caller_stack;
        // APPLY-STEP: entry reserves eight bytes per slot; checked multiplication overflow,
        // subtraction underflow or going below the floor is `stack_overflow` before any budget test.
        // An entry that does not fit is the same fault for every argument value, so the start is still
        // one basis state.
        match self.reserve(self.caller_stack, self.entry) {
            Some(stack) => initial[STACK] = stack,
            None => {
                initial[STACK] = self.caller_stack;
                initial[STATUS] = 2;
                initial[FAULT] = Fault::StackOverflow.code();
            }
        }
        initial
    }

    /// The stack pointer after reserving `function`'s frame below `stack`, if it fits above the floor.
    fn reserve(&self, stack: u64, function: FuncId) -> Option<u64> {
        let bytes = SLOT_BYTES.checked_mul(self.module.function(function).slot_count() as u64)?;
        stack.checked_sub(bytes).filter(|&pointer| pointer >= self.module.static_end())
    }

    fn with_arguments(&self, machine: &mut Machine, arguments: &[u64]) {
        for (&slot, argument) in self.parameter_slots.iter().zip(arguments) {
            machine[HEADER + FRAME_HEADER + slot] ^= argument;
        }
    }

    /// Runs one definite argument value for at most `bound` ticks, outside the reversible
    /// construction, to find when it finishes. Used to choose budgets around each finish.
    pub fn finish(&self, arguments: &[u64], bound: usize) -> Finish {
        let mut machine = self.initial_machine(bound);
        self.with_arguments(&mut machine, arguments);
        let mut ticks = None;
        for tick in 0..=bound {
            if machine[STATUS] != 0 {
                ticks = Some(tick);
                break;
            }
            if tick == bound {
                break;
            }
            machine = self.ordinary_step(&machine).expect("the reference covers every instruction reached");
        }
        Finish { output: Self::output(&machine), ticks }
    }

    pub fn apply<B: Ord, const N: usize>(
        &self,
        vector: Vector<B>,
        budget: usize,
        arguments: impl Fn(&B) -> [u64; N],
        mut xor_targets: impl FnMut(&mut B, (u64, u64)),
    ) -> (Vector<B>, Outcomes<N>) {
        assert_eq!(N, self.parameter_slots.len());
        let initial = self.initial_machine(budget);
        let blank_records = vec![vec![0; initial.len()]; budget];
        let mut vector = vector
            .into_iter()
            .map(|(registers, amplitude)| {
                (Basis { registers, machine: initial.clone(), records: blank_records.clone() }, amplitude)
            })
            .collect();
        let copy_arguments =
            |basis: &mut Basis<B>| self.with_arguments(&mut basis.machine, &arguments(&basis.registers));
        vector = permute(vector, copy_arguments);
        for tick in 0..budget {
            vector = permute(vector, |b| self.tick(&mut b.machine, &mut b.records[tick], false));
        }
        let mut outcomes = BTreeMap::new();
        vector = permute(vector, |b| {
            assert_eq!(b.machine[TICKS], budget as u64, "all inputs take exactly B scheduled ticks");
            let input = arguments(&b.registers);
            let output = Self::output(&b.machine);
            if let Some(previous) = outcomes.insert(input, output) {
                assert_eq!(previous, output, "the computation does not read its targets");
            }
            xor_targets(&mut b.registers, output);
            assert_eq!(arguments(&b.registers), input, "the XOR targets are disjoint from the arguments");
        });
        for tick in (0..budget).rev() {
            vector = permute(vector, |b| self.tick(&mut b.machine, &mut b.records[tick], true));
        }
        vector = permute(vector, copy_arguments);
        let restored = vector
            .into_iter()
            .map(|(basis, amplitude)| {
                assert_eq!(basis.machine, initial, "every machine bit restored");
                assert!(basis.records.iter().flatten().all(|&word| word == 0), "every record bit restored");
                (basis.registers, amplitude)
            })
            .collect();
        (restored, outcomes)
    }

    pub fn tick(&self, machine: &mut Machine, record: &mut Machine, inverse: bool) {
        if inverse {
            std::mem::swap(machine, record);
        }
        let updated = self.next(machine);
        for (word, value) in record.iter_mut().zip(updated) {
            *word ^= value;
        }
        if !inverse {
            std::mem::swap(machine, record);
        }
    }

    /// APPLY-STEP, "Finish": a return XORs its result into `out`, a fault its code into `fault`, and
    /// a run still running at the boundary first faults `budget`.
    fn output(c: &[u64]) -> (u64, u64) {
        match c[STATUS] {
            1 => (c[RESULT], 0),
            2 => (0, c[FAULT]),
            _ => (0, Fault::Budget.code()),
        }
    }

    fn next(&self, c: &[u64]) -> Machine {
        // Invalid encodings select identity. This defines f on every bit string, rather than
        // assuming a record is a reachable machine state when the inverse evaluates it.
        self.ordinary_step(c).unwrap_or_else(|| c.to_vec())
    }

    fn word(c: &[u64], index: usize) -> Option<usize> {
        usize::try_from(c[index]).ok()
    }

    fn ordinary_step(&self, c: &[u64]) -> Option<Machine> {
        if c[STATUS] > 2 {
            return None;
        }
        let mut n = c.to_vec();
        n[TICKS] = c[TICKS].wrapping_add(1);
        if c[STATUS] != 0 {
            return Some(n); // A terminal status idles for all remaining scheduled ticks.
        }
        let frames = (c.len() - HEADER - self.module.memory_size as usize) / self.frame_size;
        let depth = Self::word(c, DEPTH).filter(|&d| d < frames)?;
        let frame = HEADER + depth * self.frame_size;
        let function = self.module.functions.get(Self::word(c, frame + FUNCTION)?)?;
        let block = function.blocks.get(Self::word(c, frame + BLOCK)?)?;
        let position = Self::word(c, frame + POSITION).filter(|&p| p <= block.insts.len())?;
        let slots = frame + FRAME_HEADER;
        let read = |op: &Operand| match op {
            Operand::Const(k) => k.value() as u64,
            Operand::Slot(s) => c[slots + s.index()],
        };
        let memory_start = HEADER + frames * self.frame_size;
        let Some(inst) = block.insts.get(position) else {
            self.terminate(c, &mut n, frame, depth, &block.term)?;
            return Some(n);
        };
        // Value instructions write a destination and advance; a fault stops before either.
        let advance = |n: &mut Machine| n[frame + POSITION] += 1;
        match inst {
            Inst::Binary { op, width, dst, lhs, rhs } => {
                let m = mask(*width)?;
                let (a, b) = (read(lhs) & m, read(rhs) & m);
                match binary(*op, *width, a, b) {
                    Ok(value) => n[slots + dst.index()] = value & m,
                    Err(fault) => {
                        stop(&mut n, fault);
                        return Some(n);
                    }
                }
                advance(&mut n);
            }
            Inst::Compare { pred, width, dst, lhs, rhs } => {
                let m = mask(*width)?;
                let (a, b) = (read(lhs) & m, read(rhs) & m);
                let (sa, sb) = (signed(a, *width), signed(b, *width));
                n[slots + dst.index()] = u64::from(match pred {
                    Predicate::Eq => a == b,
                    Predicate::Ne => a != b,
                    Predicate::Ult => a < b,
                    Predicate::Ule => a <= b,
                    Predicate::Ugt => a > b,
                    Predicate::Uge => a >= b,
                    Predicate::Slt => sa < sb,
                    Predicate::Sle => sa <= sb,
                    Predicate::Sgt => sa > sb,
                    Predicate::Sge => sa >= sb,
                });
                advance(&mut n);
            }
            Inst::Select { width, dst, cond, if_true, if_false } => {
                // "`select` reads the condition and the chosen operand (true for 1)."
                let chosen = if read(cond) == 1 { if_true } else { if_false };
                n[slots + dst.index()] = read(chosen) & mask(*width)?;
                advance(&mut n);
            }
            Inst::Cast { op, from, to, dst, value } => {
                let a = read(value) & mask(*from)?;
                n[slots + dst.index()] = match op {
                    CastOp::Zext | CastOp::Trunc => a,
                    CastOp::Sext => signed(a, *from) as u64,
                } & mask(*to)?;
                advance(&mut n);
            }
            Inst::FuncAddr { dst, func } => {
                // "`faddr` writes the function index plus one."
                n[slots + dst.index()] = u64::from(func.0) + 1;
                advance(&mut n);
            }
            Inst::Load { width, dst, address } => {
                let m = mask(*width)?;
                match self.memory_range(read(address), *width) {
                    Ok(range) => {
                        // Little-endian, masked to the width.
                        let value = range.rev().fold(0u64, |v, at| v << 8 | c[memory_start + at] & 0xff);
                        n[slots + dst.index()] = value & m;
                        advance(&mut n);
                    }
                    Err(fault) => stop(&mut n, fault),
                }
            }
            Inst::Store { width, value, address } => {
                let value = read(value) & mask(*width)?;
                match self.memory_range(read(address), *width) {
                    Ok(range) => {
                        // The whole range takes the value's low bytes, least significant first.
                        for (i, at) in range.enumerate() {
                            n[memory_start + at] = value.checked_shr(8 * i as u32).unwrap_or(0) & 0xff;
                        }
                        advance(&mut n);
                    }
                    Err(fault) => stop(&mut n, fault),
                }
            }
            Inst::Call { callee, args, .. } => self.call(c, &mut n, frame, depth, frames, *callee, args)?,
            Inst::CallIndirect { signature, target, args, .. } => {
                // "An indirect call first resolves the function address and verifies exact parameter
                // and result types, or faults `bad_indirect`." Then the frame is reserved.
                match self
                    .module
                    .function_at(read(target))
                    .filter(|&f| self.module.function(f).has_signature(signature))
                {
                    Some(callee) => self.call(c, &mut n, frame, depth, frames, callee, args)?,
                    None => stop(&mut n, Fault::BadIndirect),
                }
            }
            Inst::Effect { .. } => stop(&mut n, Fault::EffectInRegion),
            Inst::Qalloc { .. }
            | Inst::QallocBy { .. }
            | Inst::Qfree { .. }
            | Inst::Gate { .. }
            | Inst::GateAt { .. }
            | Inst::RotateBy { .. }
            | Inst::GateBy { .. }
            | Inst::Measure { .. }
            | Inst::Apply { .. } => stop(&mut n, Fault::QuantumInRegion),
            // APPLY-STEP, "Stack". None of the three changes bytes.
            Inst::Alloca { dst, size } => {
                // "`alloca` subtracts the size and rounds down to a multiple of eight. It faults
                // `stack_overflow` on underflow or below the floor." The text does not say whether the
                // floor is compared before or after rounding; the pointer that would be set is the
                // rounded one, so this reference compares that.
                let pointer = c[STACK].checked_sub(read(size)).map(|p| p & !(SLOT_BYTES - 1));
                match pointer.filter(|&p| p >= self.module.static_end()) {
                    Some(pointer) => {
                        n[STACK] = pointer;
                        n[slots + dst.index()] = pointer;
                        advance(&mut n);
                    }
                    None => stop(&mut n, Fault::StackOverflow),
                }
            }
            Inst::StackSave { dst } => {
                n[slots + dst.index()] = c[STACK];
                advance(&mut n);
            }
            Inst::StackRestore { pointer } => {
                // "any operand between the current pointer and the entry pointer minus slot bytes,
                // inclusive, or faults `bad_stack_restore`. An unsaved or unaligned pointer in range works."
                let pointer = read(pointer);
                let reserved = SLOT_BYTES * function.slot_count() as u64;
                let top = c[frame + ENTRY_STACK].checked_sub(reserved)?;
                if (c[STACK]..=top).contains(&pointer) {
                    n[STACK] = pointer;
                    advance(&mut n);
                } else {
                    stop(&mut n, Fault::BadStackRestore);
                }
            }
            // Non-local jump and return-address instructions are not yet in this reference.
            Inst::Setjmp { .. } | Inst::ReturnAddress { .. } => return None,
        }
        Some(n)
    }

    /// APPLY-STEP, "Memory": check the entire byte range for bounds, then for qubit occupancy.
    fn memory_range(&self, address: u64, width: Width) -> Result<std::ops::Range<usize>, Fault> {
        let bytes = u64::from(width.bits()).div_ceil(8);
        let end = address.checked_add(bytes).filter(|&end| end <= self.module.memory_size).ok_or(Fault::OutOfBounds)?;
        let range = address..end;
        if self.held_bytes.iter().any(|held| range.contains(held)) {
            return Err(Fault::QubitMemoryAccess);
        }
        Ok(address as usize..end as usize)
    }

    /// APPLY-STEP, "Calls": read the arguments, reserve the callee's frame below the current stack
    /// pointer (`stack_overflow` if it does not fit), zero every slot, write the parameters and enter at
    /// block zero, instruction zero. The caller stays at the call site until the return.
    #[allow(clippy::too_many_arguments, reason = "one machine transition reads all of these")]
    fn call(
        &self,
        c: &[u64],
        n: &mut Machine,
        frame: usize,
        depth: usize,
        frames: usize,
        callee: FuncId,
        args: &[Operand],
    ) -> Option<()> {
        let Some(stack) = self.reserve(c[STACK], callee) else {
            stop(n, Fault::StackOverflow);
            return Some(());
        };
        if depth + 1 >= frames {
            return None; // Not reachable within the budget: the encoding was sized for it.
        }
        let slots = frame + FRAME_HEADER;
        let next = frame + self.frame_size;
        n[next..next + self.frame_size].fill(0);
        n[next + FUNCTION] = u64::from(callee.0);
        n[next + ENTRY_STACK] = c[STACK];
        for (param, arg) in self.module.function(callee).params.iter().zip(args) {
            n[next + FRAME_HEADER + param.slot.index()] = match arg {
                Operand::Const(k) => k.value() as u64,
                Operand::Slot(s) => c[slots + s.index()],
            };
        }
        n[STACK] = stack;
        n[DEPTH] += 1;
        Some(())
    }

    fn terminate(&self, c: &[u64], n: &mut Machine, frame: usize, depth: usize, term: &Terminator) -> Option<()> {
        let slots = frame + FRAME_HEADER;
        let read = |op: &Operand| match op {
            Operand::Const(k) => k.value() as u64,
            Operand::Slot(s) => c[slots + s.index()],
        };
        match term {
            Terminator::Jump(target) => self.jump(c, n, frame, target),
            Terminator::Branch { cond, if_true, if_false } => {
                self.jump(c, n, frame, if read(cond) == 1 { if_true } else { if_false })
            }
            Terminator::Switch { width, value, cases, default } => {
                let selector = read(value) & mask(*width)?;
                let target =
                    cases.iter().find(|case| case.value == u128::from(selector)).map_or(default, |case| &case.target);
                self.jump(c, n, frame, target);
            }
            Terminator::Return(value) => {
                let value = value.as_ref().map(read);
                if depth == 0 {
                    // "Return from the applied entry sets the returned status." The entry returns an
                    // integer (validation), so a missing value is not an encoding this reaches.
                    n[STATUS] = 1;
                    n[RESULT] = value?;
                } else {
                    // "A nested return pops its frame, restores its entry pointer, writes the call's
                    // destination if present, and advances the caller."
                    let caller = frame - self.frame_size;
                    let function = self.module.functions.get(Self::word(c, caller + FUNCTION)?)?;
                    let block = function.blocks.get(Self::word(c, caller + BLOCK)?)?;
                    let destination = match block.insts.get(Self::word(c, caller + POSITION)?)? {
                        Inst::Call { dst, .. } | Inst::CallIndirect { dst, .. } => *dst,
                        _ => return None,
                    };
                    if let Some(dst) = destination {
                        n[caller + FRAME_HEADER + dst.index()] = value?;
                    }
                    n[STACK] = c[frame + ENTRY_STACK];
                    n[frame..frame + self.frame_size].fill(0);
                    n[caller + POSITION] += 1;
                    n[DEPTH] -= 1;
                }
            }
            Terminator::Exit(_) => stop(n, Fault::ExitInRegion),
            Terminator::Fault(fault) => stop(n, *fault),
            Terminator::Longjmp { .. } => return None,
        }
        Some(())
    }

    /// "Read every incoming operand before writing any target block parameter, then move to the
    /// target's start." All operands come from `c`, so the assignment is simultaneous.
    fn jump(&self, c: &[u64], n: &mut Machine, frame: usize, target: &Target) {
        let function = &self.module.functions[c[frame + FUNCTION] as usize];
        let slots = frame + FRAME_HEADER;
        n[frame + BLOCK] = u64::from(target.block.0);
        n[frame + POSITION] = 0;
        for (param, arg) in function.block(target.block).params.iter().zip(&target.args) {
            n[slots + param.slot.index()] = match arg {
                Operand::Const(k) => k.value() as u64,
                Operand::Slot(s) => c[slots + s.index()],
            };
        }
    }
}

/// A fault costs no step and finishes the run with its code.
fn stop(n: &mut Machine, fault: Fault) {
    n[STATUS] = 2;
    n[FAULT] = fault.code();
}

/// APPLY-STEP, "Values": both operands at the declared width, the low bits of the result kept by the
/// caller. Signed operands are two's complement; signed division truncates toward zero and wraps on
/// overflow; a signed remainder is zero or has the dividend's sign with magnitude below the
/// divisor's; division or remainder by zero is `divide_by_zero`; shifts by at least the width give
/// zero.
fn binary(op: BinOp, width: Width, a: u64, b: u64) -> Result<u64, Fault> {
    let bits = u64::from(width.bits());
    let (sa, sb) = (signed(a, width), signed(b, width));
    Ok(match op {
        BinOp::Add => a.wrapping_add(b),
        BinOp::Sub => a.wrapping_sub(b),
        BinOp::Mul => a.wrapping_mul(b),
        BinOp::And => a & b,
        BinOp::Or => a | b,
        BinOp::Xor => a ^ b,
        BinOp::Udiv | BinOp::Urem | BinOp::Sdiv | BinOp::Srem if b == 0 => return Err(Fault::DivideByZero),
        BinOp::Udiv => a / b,
        BinOp::Urem => a % b,
        // In i128 the quotient of any two 64-bit values is exact; keeping the low bits wraps the one
        // overflowing case, the minimum divided by minus one, back to the minimum.
        BinOp::Sdiv => (i128::from(sa) / i128::from(sb)) as u64,
        BinOp::Srem => (i128::from(sa) % i128::from(sb)) as u64,
        // The text says shifts by at least the width give zero, and names no exception for `ashr`
        // of a negative value, so this reference gives zero there too (DESIGN's emitter section
        // says the same for arithmetic right shifts).
        BinOp::Shl | BinOp::Lshr | BinOp::Ashr if b >= bits => 0,
        BinOp::Shl => a << b,
        BinOp::Lshr => a >> b,
        BinOp::Ashr => (sa >> b) as u64,
    })
}

/// The longest chain of calls from `entry`, counting each call through `call_indirect` to every
/// function of its signature; `None` when some reachable function can reach itself.
fn call_chain(module: &Module, entry: FuncId) -> Option<usize> {
    fn depth(module: &Module, function: FuncId, active: &mut Vec<FuncId>) -> Option<usize> {
        if active.contains(&function) {
            return None;
        }
        active.push(function);
        let mut deepest = 0;
        for block in &module.function(function).blocks {
            for inst in &block.insts {
                let callees: Vec<FuncId> = match inst {
                    Inst::Call { callee, .. } => vec![*callee],
                    Inst::CallIndirect { signature, .. } => (0..module.functions.len() as u32)
                        .map(FuncId)
                        .filter(|&f| module.function(f).has_signature(signature))
                        .collect(),
                    _ => Vec::new(),
                };
                for callee in callees {
                    deepest = deepest.max(1 + depth(module, callee, active)?);
                }
            }
        }
        active.pop();
        Some(deepest)
    }
    depth(module, entry, &mut Vec::new())
}

pub fn permute<B: Ord>(vector: Vector<B>, mut operation: impl FnMut(&mut B)) -> Vector<B> {
    let mut result = Vector::new();
    for (mut basis, amplitude) in vector {
        operation(&mut basis);
        assert!(result.insert(basis, amplitude).is_none(), "a permutation cannot merge basis configurations");
    }
    result
}
