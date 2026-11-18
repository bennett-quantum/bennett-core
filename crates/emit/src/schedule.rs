//! A block's depth as a max-plus transformation of the incoming wire layers.

use std::borrow::Cow;
use std::collections::HashMap;

use bennett_exec::{GateOp, RunError, Wire};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Input {
    Wire(Wire),
    Depth,
    Zero,
}

type Row = HashMap<Input, u128>;

fn include(row: &mut Row, input: Input, weight: u128) {
    row.entry(input).and_modify(|old| *old = (*old).max(weight)).or_insert(weight);
}

/// Each changed layer is a maximum of input layers plus offsets. Missing rows are identities.
#[derive(Clone, Default)]
pub(crate) struct Schedule {
    rows: HashMap<Input, Row>,
}

impl Schedule {
    pub(crate) fn allocate(&mut self, wire: Wire) {
        self.rows.insert(Input::Wire(wire), Row::from([(Input::Zero, 0)]));
    }

    pub(crate) fn release(&mut self, wire: Wire) {
        self.rows.remove(&Input::Wire(wire));
    }

    pub(crate) fn gate(&mut self, gate: &GateOp) {
        let wires = || std::iter::once(gate.target).chain(gate.controls.iter().copied()).map(Input::Wire);
        let mut result = Row::new();
        for wire in wires() {
            match self.rows.get(&wire) {
                Some(row) => {
                    for (&input, &weight) in row {
                        include(&mut result, input, weight + 1);
                    }
                }
                None => include(&mut result, wire, 1),
            }
        }
        let depth = self.rows.entry(Input::Depth).or_insert_with(|| Row::from([(Input::Depth, 0)]));
        for (&input, &weight) in &result {
            include(depth, input, weight);
        }
        for wire in wires() {
            self.rows.insert(wire, result.clone());
        }
    }

    fn squared(&self) -> Self {
        let rows = self
            .rows
            .iter()
            .map(|(&output, row)| {
                let mut result = Row::new();
                for (&middle, &outer) in row {
                    match self.rows.get(&middle) {
                        Some(inner) => {
                            for (&input, &weight) in inner {
                                include(&mut result, input, outer + weight);
                            }
                        }
                        None => include(&mut result, middle, outer),
                    }
                }
                (output, result)
            })
            .collect();
        Self { rows }
    }

    pub(crate) fn apply_repeated(
        &self,
        mut times: u64,
        layers: &mut HashMap<Wire, u128>,
        depth: &mut u128,
    ) -> Result<(), RunError> {
        // Stage just the participating wires so an overflow cannot leave a partial call counted.
        let mut changed: HashMap<_, _> = self
            .rows
            .keys()
            .filter_map(|output| match *output {
                Input::Wire(wire) => Some((wire, layers[&wire])),
                Input::Depth | Input::Zero => None,
            })
            .collect();
        let mut latest = *depth;
        let mut schedule = Cow::Borrowed(self);
        while times != 0 {
            if times & 1 != 0 {
                schedule.apply(&mut changed, &mut latest)?;
            }
            times >>= 1;
            if times != 0 {
                schedule = Cow::Owned(schedule.squared());
            }
        }
        layers.extend(changed);
        *depth = latest;
        Ok(())
    }

    fn apply(&self, layers: &mut HashMap<Wire, u128>, depth: &mut u128) -> Result<(), RunError> {
        // Every row must read the same incoming layers, before any of them are updated.
        let mut updates = Vec::with_capacity(self.rows.len());
        for (&output, row) in &self.rows {
            let mut latest = 0;
            for (&input, &offset) in row {
                let previous = match input {
                    Input::Wire(wire) => layers[&wire],
                    Input::Depth => *depth,
                    Input::Zero => 0,
                };
                latest = latest.max(previous.checked_add(offset).ok_or(RunError::Device("gate depth overflow"))?);
            }
            updates.push((output, latest));
        }
        for (output, layer) in updates {
            match output {
                Input::Wire(wire) => {
                    layers.insert(wire, layer);
                }
                Input::Depth => *depth = layer,
                Input::Zero => unreachable!("zero is an input only"),
            }
        }
        Ok(())
    }
}
