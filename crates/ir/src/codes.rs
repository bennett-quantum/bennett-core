//! The numbers a run-time gate kind names (`docs/SYNTAX.md`, "Gate kinds at run time"): the gate codes and
//! the rotation codes of the handle interface (`docs/HANDLES.md`, 5.1), the same in its C header and the
//! device, and kept here and nowhere else in the device. A number outside its set is the fault `bad_gate`.

use crate::{FixedGate, Rotation};

/// The gate codes, in code order: the fixed gate each names.
pub const GATE_CODES: [(u64, FixedGate); 8] = [
    (1, FixedGate::X),
    (2, FixedGate::Y),
    (3, FixedGate::Z),
    (4, FixedGate::H),
    (5, FixedGate::S),
    (6, FixedGate::Sdg),
    (7, FixedGate::T),
    (8, FixedGate::Tdg),
];

/// The rotation codes, in code order: the rotation each names.
pub const ROTATION_CODES: [(u64, Rotation); 4] =
    [(9, Rotation::Rx), (10, Rotation::Ry), (11, Rotation::Rz), (12, Rotation::P)];

/// The fixed gate `code` names, if it is a gate code.
pub fn gate_code(code: u64) -> Option<FixedGate> {
    GATE_CODES.iter().find(|&&(number, _)| number == code).map(|&(_, gate)| gate)
}

/// The rotation `code` names, if it is a rotation code.
pub fn rotation_code(code: u64) -> Option<Rotation> {
    ROTATION_CODES.iter().find(|&&(number, _)| number == code).map(|&(_, rotation)| rotation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codes_are_distinct_and_name_every_gate_and_rotation_once() {
        let mut numbers: Vec<u64> =
            GATE_CODES.map(|(n, _)| n).into_iter().chain(ROTATION_CODES.map(|(n, _)| n)).collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers, (1..=12).collect::<Vec<u64>>());
        for &gate in FixedGate::ALL {
            assert_eq!(GATE_CODES.iter().filter(|&&(_, coded)| coded == gate).count(), 1, "{gate:?}");
        }
        for &rotation in Rotation::ALL {
            assert_eq!(ROTATION_CODES.iter().filter(|&&(_, coded)| coded == rotation).count(), 1, "{rotation:?}");
        }
        assert_eq!((gate_code(0), gate_code(9), rotation_code(8), rotation_code(13)), (None, None, None, None));
        assert_eq!((gate_code(2), rotation_code(12)), (Some(FixedGate::Y), Some(Rotation::P)));
    }
}
