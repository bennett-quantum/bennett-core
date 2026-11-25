//! Encode and validate the four words specified in SYNTAX and APPLY-STEP.

use bennett_ir::{BlockId, Fault, Inst, Operand, Terminator};

use super::Device;

const RECORD_BYTES: u64 = 32;

impl Device<'_> {
    pub fn next_is_nonlocal_jump(&self) -> bool {
        if self.steps >= self.budget {
            return false;
        }
        match self.instruction() {
            Some(inst) => matches!(inst, Inst::Setjmp { .. }),
            None => {
                let frame = self.frame();
                matches!(self.module.function(frame.function).block(frame.block).term, Terminator::Longjmp { .. })
            }
        }
    }

    fn suspended_depth(&self) -> usize {
        self.applied.as_ref().map_or(0, |context| context.callers.len())
    }

    pub(super) fn save_jump_record(&mut self, buffer: Operand) -> Result<(), Fault> {
        let range = self.ordinary_bytes(buffer, RECORD_BYTES)?;
        let frame = self.frame();
        let depth = self.suspended_depth() + self.frames.len() - 1;
        let words = [
            pack_pair(depth as u32, frame.function.0),
            pack_pair(frame.block.0, frame.position as u32),
            frame.entry_stack,
            self.stack,
        ];
        let (chunks, _) = self.memory[range].as_chunks_mut::<8>();
        for (chunk, word) in chunks.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        Ok(())
    }

    pub(super) fn resume_jump_record(&mut self, buffer: Operand, value: Operand) -> Result<(), Fault> {
        let range = self.ordinary_bytes(buffer, RECORD_BYTES)?;
        let mut words = [0; 4];
        let (chunks, _) = self.memory[range].as_chunks::<8>();
        for (word, chunk) in words.iter_mut().zip(chunks) {
            *word = u64::from_le_bytes(*chunk);
        }
        let [frame, point, entry_stack, stack] = words;
        let (depth, function) = unpack_pair(frame);
        // Record depths include suspended callers. Only frames belonging to this run can be resumed.
        let index = (depth as usize).checked_sub(self.suspended_depth()).ok_or(Fault::BadLongjmp)?;
        let target = self.frames.get(index).ok_or(Fault::BadLongjmp)?;
        if target.function.0 != function || target.entry_stack != entry_stack {
            return Err(Fault::BadLongjmp);
        }
        let (block, position) = unpack_pair(point);
        let block = BlockId(block);
        let position = position as usize;
        let instruction =
            self.module.function(target.function).blocks.get(block.index()).and_then(|b| b.insts.get(position));
        let Some(&Inst::Setjmp { dst, .. }) = instruction else {
            return Err(Fault::BadLongjmp);
        };
        let lower = self.frames.get(index + 1).map_or(self.stack, |callee| callee.entry_stack);
        if !(lower..=target.stack_ceiling).contains(&stack) {
            return Err(Fault::BadLongjmp);
        }

        let value = self.read(value);
        self.frames.truncate(index + 1);
        self.stack = stack;
        let target = self.frames.last_mut().expect("validated jump target");
        target.block = block;
        target.position = position + 1;
        target.slots[dst.index()] = value;
        Ok(())
    }
}

fn pack_pair(low: u32, high: u32) -> u64 {
    u64::from(low) | (u64::from(high) << 32)
}

fn unpack_pair(word: u64) -> (u32, u32) {
    (word as u32, (word >> 32) as u32)
}
