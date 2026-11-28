//! Block-form `apply` (`Inst::ApplyBlock`) from `docs/HANDLES.md` section 6 and `docs/DESIGN.md`.

use std::collections::{BTreeMap, BTreeSet};

use bennett_exec::ApplyReport;
use bennett_ir::{BlockApply, CONTROL_ENTRY_BYTES, FIELD_ENTRY_BYTES, FIELD_IN, FIELD_OUT, Fault, FuncId};

use super::{AppliedContext, Basis, Device, Finish};

#[derive(Clone, Copy)]
struct Field {
    register: usize,
    low: u32,
    bits: u32,
    offset: u64,
    byte_len: u64,
    direction: u64,
}

enum BlockOutcome {
    Returned(Vec<Vec<u8>>),
    Faulted(u64),
}

fn slices_overlap(a_reg: usize, a_low: u32, a_bits: u32, b_reg: usize, b_low: u32, b_bits: u32) -> bool {
    if a_reg != b_reg {
        return false;
    }
    let a_start = u64::from(a_low);
    let a_end = a_start + u64::from(a_bits);
    let b_start = u64::from(b_low);
    let b_end = b_start + u64::from(b_bits);
    a_start < b_end && b_start < a_end
}

impl Device<'_> {
    pub(super) fn apply_block(&mut self, apply: &BlockApply) -> Result<(), Fault> {
        // 1. Apply target check (`applied func`), giving `bad_indirect` (4).
        let callee = self
            .module
            .function_at(self.read_i64(apply.function))
            .filter(|&id| self.module.function(id).applied)
            .ok_or(Fault::BadIndirect)?;

        // 2. Fault register handle (`dangling_register`, 9) then width >= 5 (`bad_range`, 22).
        let fault_reg = self.register_operand(apply.fault)?;
        if self.quantum.live[fault_reg].1.count() < Fault::CODE_WIDTH.bits() {
            return Err(Fault::BadRange);
        }

        // 3. Size and byte-count overflow (`bad_argument`, 24).
        let size = self.read_i64(apply.size);
        if size == 0 || size > self.module.memory_size {
            return Err(Fault::BadArgument);
        }
        let field_count = self.read_i64(apply.field_count);
        let fields_bytes = field_count.checked_mul(FIELD_ENTRY_BYTES).ok_or(Fault::BadArgument)?;
        let control_count = self.read_i64(apply.control_count);
        let _controls_bytes = control_count.checked_mul(CONTROL_ENTRY_BYTES).ok_or(Fault::BadArgument)?;

        // 4. Image and field list readable (`bad_argument`), then control list (5.4).
        let image_range = self.readable_ordinary_bytes(self.read_i64(apply.image), size)?;
        let fields_range = self.readable_ordinary_bytes(self.read_i64(apply.fields), fields_bytes)?;
        let raw_controls = self.read_control_list(apply.controls, apply.control_count)?;
        let controls: Vec<_> = raw_controls.into_iter().collect::<BTreeSet<_>>().into_iter().collect();

        // 5. Each field in list order (`bad_argument` throughout).
        let mut fields = Vec::with_capacity(field_count as usize);
        for chunk in self.memory[fields_range].as_chunks::<{ FIELD_ENTRY_BYTES as usize }>().0 {
            let handle = u64::from_le_bytes(chunk[0..8].try_into().expect("field handle"));
            let low = u64::from_le_bytes(chunk[8..16].try_into().expect("field low"));
            let bits = u64::from_le_bytes(chunk[16..24].try_into().expect("field bits"));
            let offset = u64::from_le_bytes(chunk[24..32].try_into().expect("field offset"));
            let direction = u64::from_le_bytes(chunk[32..40].try_into().expect("field direction"));
            if direction != FIELD_IN && direction != FIELD_OUT {
                return Err(Fault::BadArgument);
            }
            let register = self.quantum.position(handle).ok_or(Fault::BadArgument)?;
            if bits == 0 {
                return Err(Fault::BadArgument);
            }
            let width = u64::from(self.quantum.live[register].1.count());
            let end_bit = low.checked_add(bits).ok_or(Fault::BadArgument)?;
            if end_bit > width {
                return Err(Fault::BadArgument);
            }
            let byte_len = bits.div_ceil(8);
            let end_byte = offset.checked_add(byte_len).ok_or(Fault::BadArgument)?;
            if end_byte > size {
                return Err(Fault::BadArgument);
            }
            fields.push(Field { register, low: low as u32, bits: bits as u32, offset, byte_len, direction });
        }

        // 6. Disjointness checks (`bad_argument` throughout).
        let mut byte_spans: Vec<(u64, u64)> = fields.iter().map(|f| (f.offset, f.offset + f.byte_len)).collect();
        byte_spans.sort_unstable();
        if byte_spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(Fault::BadArgument);
        }
        let fault_bits = Fault::CODE_WIDTH.bits();
        for (i, f) in fields.iter().enumerate() {
            if slices_overlap(f.register, f.low, f.bits, fault_reg, 0, fault_bits) {
                return Err(Fault::BadArgument);
            }
            for g in &fields[i + 1..] {
                if (f.direction == FIELD_OUT || g.direction == FIELD_OUT)
                    && slices_overlap(f.register, f.low, f.bits, g.register, g.low, g.bits)
                {
                    return Err(Fault::BadArgument);
                }
            }
        }
        for &(c_reg, c_bit) in &controls {
            if c_reg == fault_reg && c_bit < fault_bits {
                return Err(Fault::BadArgument);
            }
            if fields
                .iter()
                .filter(|f| f.direction == FIELD_OUT)
                .any(|f| slices_overlap(f.register, f.low, f.bits, c_reg, c_bit, 1))
            {
                return Err(Fault::BadArgument);
            }
        }

        // Snapshot and fixed placement rule (D12): highest 16-byte-aligned base with
        // static_end <= base and base + size <= stack, and no held-qubit byte in [base, base + size).
        let image = self.memory[image_range].to_vec();
        let budget = self.read_i64(apply.budget);
        let base = self
            .stack
            .checked_sub(size)
            .map(|top| top & !15)
            .filter(|&base| base >= self.module.static_end())
            .filter(|&base| self.quantum.cells.range(base..base + size).next().is_none());

        let in_fields: Vec<Field> = fields.iter().copied().filter(|f| f.direction == FIELD_IN).collect();
        let out_fields: Vec<Field> = fields.iter().copied().filter(|f| f.direction == FIELD_OUT).collect();
        let controls_active = |basis: &Basis| controls.iter().all(|&(reg, bit)| basis[reg].bit(bit));
        let input_key = |basis: &Basis| -> Vec<Vec<u8>> {
            in_fields.iter().map(|f| basis[f.register].read_slice_bytes(f.low, f.bits)).collect()
        };

        // Evaluate each distinct input once across active basis configurations.
        let mut outputs: BTreeMap<Vec<Vec<u8>>, BlockOutcome> = BTreeMap::new();
        let mut largest: Option<u64> = None;
        let mut at_budget = 0u64;
        for (row, col) in self.quantum.density.keys() {
            for basis in [row, col] {
                if !controls_active(basis) {
                    continue;
                }
                let key = input_key(basis);
                if outputs.contains_key(&key) {
                    continue;
                }
                let (outcome, steps) = match base {
                    None => (BlockOutcome::Faulted(Fault::OutOfMemory.code()), Some(0)),
                    Some(base) => self.private_block_run(callee, base, budget, &image, &in_fields, &key, &out_fields),
                };
                match steps {
                    Some(count) => largest = Some(largest.map_or(count, |best| best.max(count))),
                    None => at_budget += 1,
                }
                outputs.insert(key, outcome);
            }
        }

        self.apply_count += 1;
        self.apply_reports.push((
            ApplyReport {
                position: self.apply_count,
                callee,
                budget,
                inputs: outputs.len() as u64,
                largest,
                at_budget,
            },
            true,
        ));

        self.quantum.permute(|mut basis| {
            if controls_active(&basis) {
                let key = input_key(&basis);
                match &outputs[&key] {
                    BlockOutcome::Returned(out_bytes) => {
                        for (field, bytes) in out_fields.iter().zip(out_bytes) {
                            basis[field.register].xor_slice_bytes(field.low, field.bits, bytes);
                        }
                    }
                    BlockOutcome::Faulted(code) => {
                        for bit in 0..fault_bits {
                            if (code >> bit) & 1 != 0 {
                                basis[fault_reg].flip(bit);
                            }
                        }
                    }
                }
            }
            basis
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn private_block_run(
        &self,
        callee: FuncId,
        base: u64,
        budget: u64,
        image: &[u8],
        in_fields: &[Field],
        in_values: &[Vec<u8>],
        out_fields: &[Field],
    ) -> (BlockOutcome, Option<u64>) {
        let mut memory = self.memory.clone();
        memory[base as usize..base as usize + image.len()].copy_from_slice(image);
        for (field, bytes) in in_fields.iter().zip(in_values) {
            let start = (base + field.offset) as usize;
            memory[start..start + bytes.len()].copy_from_slice(bytes);
        }
        let mut private = Device::start(
            self.module,
            callee,
            &[u128::from(base)],
            budget,
            base,
            memory,
            Some(AppliedContext { callers: &self.frames, cells: &self.quantum.cells }),
        );
        if let Some(Finish::Faulted(fault)) = private.finish {
            return (BlockOutcome::Faulted(fault.code()), Some(0));
        }
        loop {
            if private.steps >= budget {
                return (BlockOutcome::Faulted(Fault::Budget.code()), None);
            }
            private.step(None);
            match private.finish {
                Some(Finish::Returned(None)) => {
                    let out_bytes = out_fields
                        .iter()
                        .map(|f| {
                            let start = (base + f.offset) as usize;
                            private.memory[start..start + f.byte_len as usize].to_vec()
                        })
                        .collect();
                    return (BlockOutcome::Returned(out_bytes), Some(private.steps));
                }
                Some(Finish::Faulted(fault)) => {
                    return (BlockOutcome::Faulted(fault.code()), Some(private.steps + 1));
                }
                Some(Finish::Returned(Some(_))) => panic!("applied func has no result"),
                None => {}
            }
        }
    }
}
