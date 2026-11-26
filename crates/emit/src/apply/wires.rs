//! Temporary wire numbering and final placement of an applied circuit.

use std::ops::Range;

use bennett_exec::{GateBlock, Wire};

use super::bits::Circuit;
use super::{ApplyError, ApplyInput, Argument};

struct Span {
    physical: Range<Wire>,
    local_start: Wire,
}

pub(super) struct Layout {
    interface: Vec<Span>,
    local_scratch: Wire,
    physical_scratch: Wire,
}

impl Layout {
    pub(super) fn new(input: &ApplyInput<'_>) -> Self {
        let mut ranges = vec![input.out.clone(), input.fault.clone()];
        ranges.extend(input.args.iter().filter_map(|arg| match arg {
            Argument::Register(wires) => Some(wires.clone()),
            Argument::Value(_) => None,
        }));
        ranges.sort_unstable_by_key(|range| range.start);
        let mut interface: Vec<Span> = Vec::new();
        let mut local_scratch = 0;
        // One offset maps every wire in a span. Merging ranges keeps overlapping
        // argument slices on the same wires, without enumerating either slice.
        for range in ranges {
            if let Some(last) = interface.last_mut()
                && range.start <= last.physical.end
            {
                local_scratch += range.end.saturating_sub(last.physical.end);
                last.physical.end = last.physical.end.max(range.end);
            } else {
                let width = range.end - range.start;
                interface.push(Span { physical: range, local_start: local_scratch });
                local_scratch += width;
            }
        }
        Self { interface, local_scratch, physical_scratch: input.scratch_start }
    }

    pub(super) fn arguments(&self, args: &[Argument]) -> Vec<Argument> {
        args.iter()
            .map(|arg| match arg {
                Argument::Register(wires) => Argument::Register(self.local(wires.clone())),
                Argument::Value(value) => Argument::Value(*value),
            })
            .collect()
    }

    pub(super) fn local(&self, wires: Range<Wire>) -> Range<Wire> {
        let end = self.interface.partition_point(|span| span.physical.start <= wires.start);
        let span = &self.interface[end - 1];
        let start = span.local_start + (wires.start - span.physical.start);
        start..start + (wires.end - wires.start)
    }

    pub(super) fn scratch_start(&self) -> Wire {
        self.local_scratch
    }

    pub(super) fn fits(&self, circuit: &Circuit) -> bool {
        circuit.fits_at(self.physical_scratch)
    }

    pub(super) fn finish(&self, circuit: Circuit, forward: usize, name: &str) -> Result<GateBlock, ApplyError> {
        if !self.fits(&circuit) {
            return Err(ApplyError::WireSpace);
        }
        // All interface ranges lie below physical scratch. Equal starts mean
        // the packed ranges already cover that whole interval without gaps.
        if self.physical_scratch == self.local_scratch {
            return circuit.finish(forward, name);
        }
        circuit.finish_placed(forward, name, |wire| {
            if wire < self.local_scratch {
                let end = self.interface.partition_point(|span| span.local_start <= wire);
                let span = &self.interface[end - 1];
                span.physical.start + (wire - span.local_start)
            } else {
                self.physical_scratch + (wire - self.local_scratch)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bennett_exec::BlockOp;
    use bennett_ir::load;

    use super::super::bits::{Bit, ONE};

    #[test]
    fn temporary_wire_layout_counts_spans_instead_of_wires() {
        let module = load("func @f(i2 %a, i2 %b) -> i1 {\nentry:\n    ret 0\n}\n").expect("valid module");
        let mut entries = Vec::new();
        for width in [5, 1 << 20] {
            let fault = 1..1 + width;
            let first_arg = fault.end + 16;
            let left = first_arg..first_arg + 2;
            let right = first_arg + 1..first_arg + 3;
            let input = ApplyInput {
                module: &module,
                callee: module.function_named("f").expect("applied function"),
                args: &[Argument::Register(left.clone()), Argument::Register(right.clone())],
                out: 0..1,
                fault: fault.clone(),
                budget: 1,
                entry_fault: None,
                stack_available: 256,
                memory: None,
                scratch_start: Wire::MAX - 1,
            };
            let layout = Layout::new(&input);
            entries.push(layout.interface.len());
            assert_eq!(layout.local(fault.clone()), fault);
            assert_eq!(layout.local(left.clone()), fault.end..fault.end + 2);
            assert_eq!(layout.local(right), fault.end + 1..fault.end + 3);

            let mut circuit = Circuit::new(layout.scratch_start());
            let temporary = circuit.fresh();
            let source = Bit::Wire(layout.local(left).start, false);
            circuit.xor_into(temporary, source, ONE);
            let forward = circuit.forward_len();
            circuit.xor_into(layout.local(fault.clone()).start, Bit::Wire(temporary, false), ONE);

            let block = layout.finish(circuit, forward, "sparse").expect("fits at the end of wire space");
            let [
                BlockOp::Allocate(scratch),
                BlockOp::Gate(compute),
                BlockOp::Gate(copy),
                BlockOp::Gate(undo),
                BlockOp::ReleaseZero(release),
            ] = block.ops()
            else {
                panic!("expected one compute, copy and undo");
            };
            assert_eq!(*scratch, Wire::MAX - 1..Wire::MAX);
            assert_eq!(scratch, release);
            assert_eq!((compute.target, compute.controls.as_slice()), (scratch.start, &[first_arg][..]));
            assert_eq!((copy.target, copy.controls.as_slice()), (fault.start, &[scratch.start][..]));
            assert_eq!(compute, undo);
        }
        assert_eq!(entries[0], entries[1], "layout entries must not grow with fault width");
        assert_eq!(entries[0], 2, "two disjoint spans after merging adjacent and overlapping ranges");
    }
}
