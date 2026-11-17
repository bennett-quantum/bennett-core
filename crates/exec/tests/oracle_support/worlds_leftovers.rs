//! DESIGN, Strict mode: logical snapshots exclude physical columns retained by an inverse.
//! A retained allocation/promotion returns at its original step; a read returns its copy XOR
//! the current source and clears its separate wire. Neither quantum values nor amplitudes are
//! saved in the routing metadata. This is independent of production journals and allocators.
use super::{Classical, Model, Register, Value, World};
use bennett_ir::{Operand, Type};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Allocation,
    Promotion { address: u64, old: u8 },
    Read { column: usize },
    Cell { column: usize },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Retained {
    pub register: Register,
    pub boundary: usize,
    pub kind: Kind,
}

impl World {
    pub fn physical_registers(&self) -> Vec<Register> {
        let mut registers = self.classical.registers.clone();
        for (&id, retained) in &self.retained {
            registers.resize_with(registers.len().max(id + 1), || Register {
                width: None,
                generation: 0,
                allocated_at: 0,
            });
            registers[id] = retained.register.clone();
        }
        registers
    }

    pub(super) fn trim_quantum(&mut self) {
        let length = self.retained.keys().next_back().map_or(0, |id| id + 1).max(self.classical.registers.len());
        self.quantum.truncate(length);
    }

    pub(super) fn retain_allocation(&mut self, id: usize, kind: Kind) {
        let mut register = self.classical.registers[id].clone();
        register.generation += 1;
        self.retained.insert(id, Retained { register, boundary: self.history.len(), kind });
    }

    pub(super) fn retain_wire(&mut self, column: usize, value: u64, cell: bool) {
        let registers = self.physical_registers();
        let id = registers.iter().position(|r| r.width.is_none()).unwrap_or(registers.len());
        let register = Register {
            width: self.classical.registers[column].width,
            generation: self.classical.registers.get(id).map_or(2, |r| r.generation + 2),
            allocated_at: 0,
        };
        self.quantum.resize(self.quantum.len().max(id + 1), 0);
        self.quantum[id] = value ^ self.quantum[column];
        self.retained.insert(
            id,
            Retained {
                register,
                boundary: self.history.len(),
                kind: if cell { Kind::Cell { column } } else { Kind::Read { column } },
            },
        );
    }

    fn take(&mut self, kind: &Kind) -> Option<(usize, Register)> {
        let id = self
            .retained
            .iter()
            .find(|(_, r)| r.boundary == self.history.len() && &r.kind == kind)
            .map(|(&id, _)| id)?;
        let mut r = self.retained.remove(&id).expect("retained step").register;
        r.generation -= 1;
        self.classical.time.leftovers.remove(&id);
        Some((id, r))
    }

    fn take_wire(&mut self, column: usize, cell: bool) -> Option<u64> {
        let kind = if cell { Kind::Cell { column } } else { Kind::Read { column } };
        let (id, _) = self.take(&kind)?;
        let value = self.quantum[id] ^ self.quantum[column];
        self.quantum[id] = 0;
        self.trim_quantum();
        Some(value)
    }

    pub(super) fn restore_classical(&mut self, before: &Classical) {
        self.classical = before.clone();
        self.classical.time.leftovers.extend(self.retained.keys());
        self.trim_quantum();
    }
}

impl Model<'_> {
    pub(super) fn allocate(&self, w: &mut World, width: u32) -> (usize, u64) {
        self.allocate_kind(w, width, Kind::Allocation)
    }

    pub(super) fn allocate_kind(&self, w: &mut World, width: u32, kind: Kind) -> (usize, u64) {
        if self.replaying.get() > 0
            && let Some((id, info)) = w.take(&kind)
        {
            let generation = info.generation;
            assert_eq!(info.width, Some(width));
            if id == w.classical.registers.len() {
                w.classical.registers.push(info);
            } else {
                w.classical.registers[id] = info;
            }
            return (id, generation);
        }
        let id = (0..)
            .find(|id| !w.retained.contains_key(id) && w.classical.registers.get(*id).is_none_or(|r| r.width.is_none()))
            .expect("finite table");
        let generation = w.classical.registers.get(id).map_or(1, |r| r.generation + 1);
        let info = Register { width: Some(width), generation, allocated_at: w.history.len() };
        while w.classical.registers.len() <= id {
            w.classical.registers.push(Register { width: None, generation: 0, allocated_at: 0 });
        }
        w.classical.registers[id] = info;
        w.quantum.resize(w.quantum.len().max(id + 1), 0);
        w.quantum[id] = 0;
        (id, generation)
    }

    pub(super) fn arguments(
        &self,
        w: &mut World,
        args: &[Operand],
        params: &[Type],
    ) -> (Vec<Value>, Vec<(usize, u64)>) {
        let mut reads = Vec::new();
        let values = args
            .iter()
            .zip(params)
            .map(|(arg, ty)| match (w.read(*arg), ty) {
                (Value::Register(id, g), Type::Int(_)) => {
                    // A fresh call still validates the generation in call(); replay has the same handle.
                    if w.live(id, g).is_err() {
                        return Value::Register(id, g);
                    }
                    let value = if self.replaying.get() > 0 {
                        w.take_wire(id, false).unwrap_or(w.quantum[id])
                    } else {
                        w.quantum[id]
                    };
                    reads.push((id, value));
                    Value::Int(value.into())
                }
                (value, _) => value,
            })
            .collect();
        (values, reads)
    }

    pub(super) fn access_memory(
        &self,
        w: &mut World,
        address: u64,
        bytes: usize,
    ) -> Result<Vec<u8>, bennett_ir::Fault> {
        let mut values = self.read_memory(w, address, bytes)?;
        if self.replaying.get() > 0 {
            for (j, byte) in values.iter_mut().enumerate() {
                if let Some(&column) = w.classical.cells.get(&(address + j as u64))
                    && let Some(value) = w.take_wire(column, true)
                {
                    *byte = value as u8;
                }
            }
        }
        Ok(values)
    }

    pub(super) fn store_access(
        &self,
        w: &mut World,
        address: u64,
        bytes: &[u8],
    ) -> Result<Vec<(usize, u8, u8)>, bennett_ir::Fault> {
        let old = self.access_memory(w, address, bytes.len())?;
        let cells =
            w.classical.cells.range(address..address + bytes.len() as u64).map(|(&a, &id)| (a, id)).collect::<Vec<_>>();
        let current = cells.iter().map(|&(_, id)| w.quantum[id]).collect::<Vec<_>>();
        self.write_memory(w, address, bytes)?;
        let mut noted = Vec::new();
        for ((a, id), current) in cells.into_iter().zip(current) {
            let j = (a - address) as usize;
            w.quantum[id] = current ^ u64::from(old[j]) ^ u64::from(bytes[j]);
            noted.push((id, old[j], bytes[j]));
        }
        Ok(noted)
    }
}
