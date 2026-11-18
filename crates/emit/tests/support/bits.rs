//! Test definite states in batches of 64. Word bit n on every wire is input n.

use bennett_exec::{GateOp, Wire};
use bennett_ir::{FixedGate, Gate};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// The widths of adjacent x, y and scratch registers, starting at wire zero.
#[derive(Clone, Copy)]
pub struct Layout {
    pub x: usize,
    pub y: usize,
    pub scratch: usize,
}

/// Run X, CNOT and Toffoli gates in place. The resulting slice contains the output words.
pub fn run(gates: &[GateOp], wires: &mut [u64]) -> Result<(), String> {
    for (index, gate) in gates.iter().enumerate() {
        if gate.kind != Gate::Fixed(FixedGate::X) || gate.controls.len() > 2 {
            return Err(format!("gate {index} is outside X, CNOT and Toffoli"));
        }
        let wire_index = |wire: Wire| {
            usize::try_from(wire)
                .ok()
                .filter(|&wire| wire < wires.len())
                .ok_or_else(|| format!("gate {index} names missing wire {wire}"))
        };
        let target = wire_index(gate.target)?;
        let mut flip = u64::MAX;
        for (position, &control) in gate.controls.iter().enumerate() {
            if control == gate.target || gate.controls[..position].contains(&control) {
                return Err(format!("gate {index} repeats wire {control}"));
            }
            flip &= wires[wire_index(control)?];
        }
        wires[target] ^= flip;
    }
    Ok(())
}

/// Check every pair of input values for a small layout.
pub fn exhaustive(gates: &[GateOp], layout: Layout, f: impl Fn(u128) -> u128) -> Result<(), String> {
    let domain = |width| {
        u32::try_from(width)
            .ok()
            .and_then(|width| 1u128.checked_shl(width))
            .ok_or_else(|| "register too wide to exhaust".to_owned())
    };
    let x_values = domain(layout.x)?;
    let y_values = domain(layout.y)?;
    check_states(gates, layout, (0..x_values).flat_map(|x| (0..y_values).map(move |y| (x, y))), f)
}

/// Check seeded samples of x and y. Each register may contain up to 128 bits.
pub fn sampled(
    gates: &[GateOp],
    layout: Layout,
    samples: usize,
    seed: u64,
    f: impl Fn(u128) -> u128,
) -> Result<(), String> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let x_mask = mask(layout.x)?;
    let y_mask = mask(layout.y)?;
    let inputs = (0..samples).map(|_| (rng.random::<u128>() & x_mask, rng.random::<u128>() & y_mask));
    check_states(gates, layout, inputs, f)
}

fn mask(width: usize) -> Result<u128, String> {
    if width > 128 {
        return Err("register exceeds 128 bits".to_owned());
    }
    Ok(u128::MAX.checked_shr((128 - width) as u32).unwrap_or(0))
}

fn pack(wires: &mut [u64], value: u128, lane: usize) {
    for (bit, word) in wires.iter_mut().enumerate() {
        *word |= ((value >> bit & 1) as u64) << lane;
    }
}

/// Check a chosen sequence of input and initial destination words.
pub fn check_states(
    gates: &[GateOp],
    layout: Layout,
    mut inputs: impl Iterator<Item = (u128, u128)>,
    f: impl Fn(u128) -> u128,
) -> Result<(), String> {
    let input_width = layout.x + layout.y;
    let mut wires = vec![0; input_width + layout.scratch];
    let mut expected = vec![0; input_width];
    let y_mask = mask(layout.y)?;
    let mut batch = Vec::with_capacity(64);

    loop {
        batch.clear();
        batch.extend(inputs.by_ref().take(64));
        if batch.is_empty() {
            return Ok(());
        }
        wires.fill(0);
        expected.fill(0);
        for (lane, &(x, y)) in batch.iter().enumerate() {
            let result = f(x);
            if result & !y_mask != 0 {
                return Err(format!("f({x})={result} exceeds the output width"));
            }
            pack(&mut wires[..layout.x], x, lane);
            pack(&mut wires[layout.x..input_width], y, lane);
            pack(&mut expected[..layout.x], x, lane);
            pack(&mut expected[layout.x..], y ^ result, lane);
        }
        run(gates, &mut wires)?;

        let lanes = u64::MAX >> (64 - batch.len());
        for (wire, &actual) in wires.iter().enumerate() {
            let changed = (actual ^ expected.get(wire).copied().unwrap_or(0)) & lanes;
            if changed != 0 {
                let (x, y) = batch[changed.trailing_zeros() as usize];
                let register = if wire < layout.x {
                    "input"
                } else if wire < input_width {
                    "output"
                } else {
                    "scratch"
                };
                return Err(format!("{register} wire {wire} disagrees for x={x}, y={y}"));
            }
        }
    }
}
