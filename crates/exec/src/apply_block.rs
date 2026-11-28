//! `apply` to a block (`docs/SYNTAX.md`, "Applying to a block"; `docs/HANDLES.md`, section 6): the handle
//! interface's `apply`, every operand a value given at run time.
//!
//! The checks of `docs/HANDLES.md` 6.2 run in order from definite values; the first that fails ends the run
//! with its fault, before anything changes. Then the device determines a total function `G` from inputs (the
//! values of the *in* fields' qubits) to the *out* fields' values or a fault code (6.3), and applies the
//! unitary of 6.4 on the basis configurations where every control qubit is 1: the *out* values are XORed
//! into the *out* fields' qubits, a fault code into the five low qubits of the fault register. A fault of the
//! applied function on some input is such a value, never a stop of the run.
//!
//! `G(x)`, as the simulator computes it: the block's base is fixed once for the call (placement); for each
//! input, on the machine as it is at the call, the image is copied into the block and the *in* fields
//! written into their bytes (setup, no steps), the apply target is entered with the block's address and the
//! stack pointer at the block's base, runs for `budget` steps, and the *out* fields' bytes are read after it
//! returns (extraction, no steps); then the run is undone and the block's bytes put back, so nothing of it
//! remains but the XOR.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use bennett_ir::{BlockApply, FIELD_ENTRY_BYTES, FIELD_IN, FIELD_OUT, Fault, FuncId, Inst, Operand};

use crate::apply_report::{ApplyReport, RunSteps};
use crate::device::Wire;
use crate::executor::{Executor, Halt, Outcome};
use crate::quantum::{Config, QubitRef};

/// The block's base is a multiple of this (the decision of 2026-11-25).
const BLOCK_ALIGNMENT: u64 = 16;

/// The members of a field entry, in order, as byte offsets.
const FIELD_HANDLE: usize = 0;
const FIELD_LOW: usize = 8;
const FIELD_BITS: usize = 16;
const FIELD_OFFSET: usize = 24;
const FIELD_DIRECTION: usize = 32;

/// A checked field: its qubits from the lowest, its bytes in the block, its direction.
struct Field {
    qubits: Vec<Wire>,
    offset: usize,
    out: bool,
}

impl Field {
    /// The number of bytes the field occupies, `ceil(bits / 8)`.
    fn bytes(&self) -> usize {
        self.qubits.len().div_ceil(8)
    }
}

/// An `apply` whose operands passed every check.
struct Checked {
    callee: FuncId,
    budget: u64,
    /// The five low qubits of the fault register.
    fault: Vec<Wire>,
    /// The image, copied at the call (the snapshot of 6.3).
    image: Vec<u8>,
    fields: Vec<Field>,
    controls: Vec<Wire>,
}

/// What `G` gives for one input: the *out* fields' values, one bit per qubit in field order, or a fault code.
type Value = Result<Vec<bool>, Fault>;

fn word(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

impl Executor<'_> {
    /// Executes an `Inst::ApplyBlock` on the simulator (module documentation), the `position`-th `apply` of the
    /// run (`None` when it runs again to undo it: no report).
    pub(crate) fn apply_block(&mut self, inst: &Inst, position: Option<u64>) -> Result<(), Halt> {
        let Inst::ApplyBlock(apply) = inst else { unreachable!("an ApplyBlock instruction") };
        let BlockApply { function, budget, fault, image, size, fields, field_count, controls, control_count } =
            &**apply;
        let [function, budget, image, size, fields, field_count, controls, control_count] =
            [function, budget, image, size, fields, field_count, controls, control_count].map(|op| self.read_word(op));
        let checked =
            self.checked(function, budget, fault, (image, size), (fields, field_count), (controls, control_count))?;
        let base = self.placement(size);
        let qubit = |wire: Wire| self.quantum.device_wires.qubit(wire).expect("a checked qubit is bound");
        let refs = |wires: &[Wire]| wires.iter().map(|&wire| qubit(wire)).collect::<Vec<QubitRef>>();
        let controls = refs(&checked.controls);
        let fault_qubits = refs(&checked.fault);
        let ins: Vec<Vec<QubitRef>> = checked.fields.iter().filter(|f| !f.out).map(|f| refs(&f.qubits)).collect();
        let outs: Vec<QubitRef> = checked.fields.iter().filter(|f| f.out).flat_map(|f| refs(&f.qubits)).collect();

        let original = self.quantum.take_amplitudes();
        self.measure_begin_apply(checked.callee);
        let mut values: BTreeMap<Vec<bool>, Value> = BTreeMap::new();
        let mut runs = Vec::new();
        for config in original.keys() {
            if !controls.iter().all(|control| control.is_set(config)) {
                continue;
            }
            let input: Vec<bool> = ins.iter().flatten().map(|qubit| qubit.is_set(config)).collect();
            if let Entry::Vacant(slot) = values.entry(input) {
                let (value, run) = self.evaluate_block(&checked, base, &ins, slot.key(), config);
                runs.extend(run);
                slot.insert(value);
            }
        }
        if let Some(position) = position.filter(|_| self.keep_apply_reports) {
            self.apply_reports.push(ApplyReport::new(position, checked.callee, checked.budget, &runs));
        }

        let mut faulting = 0.0;
        let mut permuted = BTreeMap::new();
        for (mut config, amplitude) in original {
            if controls.iter().all(|control| control.is_set(&config)) {
                let input: Vec<bool> = ins.iter().flatten().map(|qubit| qubit.is_set(&config)).collect();
                match &values[&input] {
                    Ok(bits) => {
                        for (qubit, &bit) in outs.iter().zip(bits) {
                            let old = qubit.is_set(&config);
                            qubit.assign(&mut config, old ^ bit);
                        }
                    }
                    Err(code) => {
                        faulting += amplitude.norm_sqr();
                        for (k, qubit) in fault_qubits.iter().enumerate() {
                            let old = qubit.is_set(&config);
                            qubit.assign(&mut config, old ^ (code.code() >> k & 1 == 1));
                        }
                    }
                }
            }
            permuted.insert(config, amplitude);
        }
        if values.values().any(Result::is_err) {
            self.record_region_fault_weight(faulting);
        }
        self.quantum.replace_amplitudes(permuted);
        Ok(())
    }

    /// The checks of `docs/HANDLES.md` 6.2, in order; the first that fails is the fault.
    fn checked(
        &self,
        function: u64,
        budget: u64,
        fault: &Operand,
        (image, size): (u64, u64),
        (fields, field_count): (u64, u64),
        (controls, control_count): (u64, u64),
    ) -> Result<Checked, Fault> {
        // 1. An apply target of the program (D11).
        let callee =
            self.module.function_at(function).filter(|&f| self.function(f).applied).ok_or(Fault::BadIndirect)?;
        // 2. The fault register, then its width.
        let handle = self.handle_of(fault);
        let (_, qubits) = self.register_of(handle)?;
        let code_bits = u64::from(Fault::CODE_WIDTH.bits());
        if u64::from(qubits.count()) < code_bits {
            return Err(Fault::BadRange);
        }
        let fault = (0..code_bits).map(|bit| self.wire_of(handle, bit)).collect::<Result<Vec<_>, _>>()?;
        // 3. The size, and the lists' byte counts in the target's 64-bit arithmetic.
        let overflows = |count: u64, entry: u64| u128::from(count) * u128::from(entry) > u128::from(u64::MAX);
        if size == 0
            || size > self.module.memory_size
            || overflows(field_count, FIELD_ENTRY_BYTES)
            || overflows(control_count, bennett_ir::CONTROL_ENTRY_BYTES)
        {
            return Err(Fault::BadArgument);
        }
        // 4. The image and the field list readable, then the control list (its own checks, 5.4).
        let image = self.list_bytes(image, size, 1)?;
        let entries = self.list_bytes(fields, field_count, FIELD_ENTRY_BYTES)?;
        let controls = self.control_list(controls, control_count)?;
        // 5. Each field in list order.
        let fields = entries
            .chunks(FIELD_ENTRY_BYTES as usize)
            .map(|entry| self.field(entry, size))
            .collect::<Result<Vec<_>, _>>()?;
        // 6. No two fields' bytes overlap; no qubit in two out fields, an out and an in field, a field and the
        //    fault qubits, or the controls and an out field or the fault qubits.
        let mut spans: Vec<(usize, usize)> = fields.iter().map(|f| (f.offset, f.offset + f.bytes())).collect();
        spans.sort_unstable();
        if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(Fault::BadArgument);
        }
        let mut written = BTreeSet::new();
        for field in fields.iter().filter(|f| f.out) {
            if !field.qubits.iter().all(|&qubit| written.insert(qubit)) {
                return Err(Fault::BadArgument);
            }
        }
        let read: BTreeSet<Wire> = fields.iter().filter(|f| !f.out).flat_map(|f| f.qubits.iter().copied()).collect();
        let fault_set: BTreeSet<Wire> = fault.iter().copied().collect();
        let controls_set: BTreeSet<Wire> = controls.iter().copied().collect();
        if !written.is_disjoint(&read)
            || !written.is_disjoint(&fault_set)
            || !read.is_disjoint(&fault_set)
            || !controls_set.is_disjoint(&written)
            || !controls_set.is_disjoint(&fault_set)
        {
            return Err(Fault::BadArgument);
        }
        Ok(Checked { callee, budget, fault, image, fields, controls })
    }

    /// One field entry, checked (`bad_argument` throughout): the direction, the handle, at least one qubit,
    /// the slice inside its register and the bytes inside the block.
    fn field(&self, entry: &[u8], size: u64) -> Result<Field, Fault> {
        let [handle, low, bits, offset, direction] =
            [FIELD_HANDLE, FIELD_LOW, FIELD_BITS, FIELD_OFFSET, FIELD_DIRECTION].map(|at| word(entry, at));
        if direction != FIELD_IN && direction != FIELD_OUT {
            return Err(Fault::BadArgument);
        }
        let (_, qubits) = self.register_of(handle).map_err(|_| Fault::BadArgument)?;
        if bits == 0
            || u128::from(low) + u128::from(bits) > u128::from(qubits.count())
            || u128::from(offset) + u128::from(bits.div_ceil(8)) > u128::from(size)
        {
            return Err(Fault::BadArgument);
        }
        let qubits = (low..low + bits).map(|bit| self.wire_of(handle, bit)).collect::<Result<Vec<_>, _>>()?;
        Ok(Field { qubits, offset: offset as usize, out: direction == FIELD_OUT })
    }

    /// The block's base (6.3, placement): the highest multiple of 16 at which `size` bytes end at or below the
    /// stack pointer at the call, at or above the stack floor (the first byte above static data), with no
    /// byte holding qubits. It reads the size, the stack pointer and the stack floor only (where the bytes
    /// holding qubits are only decides whether there is a base). `None` if there is no such base: then `G` is
    /// `out_of_memory` for every input.
    fn placement(&self, size: u64) -> Option<u64> {
        let base = self.stack_pointer.checked_sub(size)? / BLOCK_ALIGNMENT * BLOCK_ALIGNMENT;
        (base >= self.stack_floor && !self.quantum.has_cells_in(base..base + size)).then_some(base)
    }

    /// `G(input)`, and the run that computed it with its step count (none if setup failed).
    fn evaluate_block(
        &mut self,
        checked: &Checked,
        base: Option<u64>,
        ins: &[Vec<QubitRef>],
        input: &[bool],
        config: &Config,
    ) -> (Value, Option<RunSteps>) {
        let Some(base) = base else { return (Err(Fault::OutOfMemory), None) };
        debug_assert_eq!(ins.iter().flatten().map(|qubit| qubit.is_set(config)).collect::<Vec<_>>(), input);
        let block = base as usize..base as usize + checked.image.len();
        let mut saved = vec![0; block.len()];
        self.memory.read_into(block.clone(), &mut saved);
        // Setup: the image, then each in field's value, least significant byte first, unused high bits 0.
        let mut bytes = checked.image.clone();
        let mut bits = input.iter();
        for field in checked.fields.iter().filter(|f| !f.out) {
            bytes[field.offset..field.offset + field.bytes()].fill(0);
            for k in 0..field.qubits.len() {
                if *bits.next().expect("one bit per in qubit") {
                    bytes[field.offset + k / 8] |= 1 << (k % 8);
                }
            }
        }
        self.memory.write(block.start, &bytes);
        let outer_stack_pointer = std::mem::replace(&mut self.stack_pointer, base);
        let outs: Vec<&Field> = checked.fields.iter().filter(|f| f.out).collect();
        let (outcome, steps, extracted) =
            self.evaluate_with(checked.callee, &[u128::from(base)], checked.budget, |executor, outcome| {
                let Outcome::Returned(_) = outcome else { return None };
                let mut bits = Vec::new();
                for field in &outs {
                    let start = block.start + field.offset;
                    if executor.quantum.has_cells_in(start as u64..(start + field.bytes()) as u64) {
                        return Some(Err(Fault::OutOfBounds));
                    }
                    let mut value = vec![0; field.bytes()];
                    executor.memory.read_into(start..start + field.bytes(), &mut value);
                    bits.extend((0..field.qubits.len()).map(|k| value[k / 8] >> (k % 8) & 1 == 1));
                }
                Some(Ok(bits))
            });
        self.stack_pointer = outer_stack_pointer;
        self.memory.write(block.start, &saved);
        let value = match outcome {
            Outcome::Returned(_) => extracted.expect("extracted after a return"),
            Outcome::Faulted(code) => Err(code),
            // A stop of the simulator concerns gates and `apply`, which fault `quantum_in_region` first.
            Outcome::Stopped(_) => unreachable!("an applied function reaches no gate or apply"),
        };
        (value, Some(steps))
    }
}
