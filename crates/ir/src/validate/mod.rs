//! The validator: everything the executor relies on, checked once.
//!
//! A [`ValidModule`] can only be made by [`validate`], so the executor may index slots, blocks and
//! functions without checking and may trust every operand's type.

mod error;
mod function;

use std::collections::HashSet;
use std::ops::Deref;

pub use error::{Expected, Location, Problem, ValidationError, ValidationErrors};

use crate::{DataItem, FuncId, Module, Slot, Type};

/// A module the validator accepted, with the type of every slot.
#[derive(Clone, Debug)]
pub struct ValidModule {
    module: Module,
    slot_types: Vec<Vec<Type>>,
}

impl ValidModule {
    /// The type of `slot` in `func`.
    pub fn slot_type(&self, func: FuncId, slot: Slot) -> Type {
        self.slot_types[func.index()][slot.index()]
    }

    /// The module, giving up the validity guarantee.
    pub fn into_module(self) -> Module {
        self.module
    }
}

impl Deref for ValidModule {
    type Target = Module;

    fn deref(&self) -> &Module {
        &self.module
    }
}

/// Checks `module`: well-formed indices, SSA (each slot defined once, every use dominated by its
/// definition), operand types, data layout, and the static rules of the quantum instructions.
pub fn validate(module: Module) -> Result<ValidModule, ValidationErrors> {
    let mut errors = Vec::new();
    check_data(&module, &mut errors);
    check_function_names(&module, &mut errors);
    let slot_types = module.functions.iter().map(|f| function::check(&module, f, &mut errors)).collect();
    if errors.is_empty() { Ok(ValidModule { module, slot_types }) } else { Err(ValidationErrors(errors)) }
}

fn check_data(module: &Module, errors: &mut Vec<ValidationError>) {
    let mut error = |index, problem| errors.push(ValidationError { location: Location::Data(index), problem });
    let mut extents = Vec::new();
    for (index, segment) in module.data.iter().enumerate() {
        let len = segment.len();
        match segment.address.checked_add(len) {
            Some(end) if end <= module.memory_size => extents.push((segment.address, end, index)),
            _ => error(
                index,
                Problem::DataOutOfBounds { address: segment.address, len, memory_size: module.memory_size },
            ),
        }
        for item in &segment.items {
            if let DataItem::FuncAddr { width, func } = item {
                match module.functions.get(func.index()) {
                    None => error(index, Problem::UnknownFunction(func.0)),
                    Some(f) if u128::from(func.address()) > width.mask() => {
                        error(index, Problem::AddressTooWide { function: f.name.clone(), bits: width.bits() });
                    }
                    Some(_) => {}
                }
            }
        }
    }
    extents.retain(|(start, end, _)| start < end);
    extents.sort_unstable();
    for pair in extents.windows(2) {
        let ((_, end, first), (start, _, second)) = (pair[0], pair[1]);
        if start < end {
            error(first.max(second), Problem::DataOverlap(first.min(second)));
        }
    }
}

fn check_function_names(module: &Module, errors: &mut Vec<ValidationError>) {
    let mut seen = HashSet::new();
    for function in &module.functions {
        if !seen.insert(function.name.as_str()) {
            let location = Location::Function(function.name.clone());
            errors.push(ValidationError { location, problem: Problem::DuplicateFunction });
        }
    }
}
