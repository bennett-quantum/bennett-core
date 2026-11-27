//! Seeded programs with wide register values and computed angles.

use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;

use bennett_ir::Fault;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, module, observe};

const WIDTHS: [u32; 8] = [63, 64, 65, 127, 128, 129, 257, 4097];
const ANGLE_CASES: usize = 12;
const DEFAULT_PROGRAMS: u64 = (WIDTHS.len() * ANGLE_CASES) as u64;
const HEAVY_PROGRAMS: u64 = 2_048;
const FIRST_SEED: u64 = 0x17a6_f30c_71e2_5f84;

fn generate(seed: u64) -> (String, u64, bool) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let case = (seed - FIRST_SEED) as usize;
    let width = WIDTHS[case % WIDTHS.len()];
    let top = width - 1;
    let interior = rng.random_range(1..top);
    let angle = match (case / WIDTHS.len()) % ANGLE_CASES {
        0 => 0.0,
        1 => -0.0,
        2 => TAU,
        3 => -TAU,
        4 => -PI,
        5 => f64::MAX,
        6 => -1e300,
        7 => f64::from_bits(1),
        8 => f64::INFINITY,
        9 => f64::NEG_INFINITY,
        10 => f64::from_bits(0x7ff0_0000_0000_0001 | (rng.random::<u64>() & 0x000f_ffff_ffff_ffff)),
        _ => rng.random_range(-8.0..8.0),
    };
    let axis = ["rx", "ry", "rz"][rng.random_range(0..3)];
    let key = rng.random::<u64>();
    let location = match rng.random_range(0..3) {
        0 => format!("%wide[{top}] ctrl %wide[0]"),
        1 => format!("%wide[{top}] ctrl %wide[0], [9:5]"),
        _ => format!("[9:5] ctrl %wide[{top}], %wide[{interior}]"),
    };
    let mut text = format!(
        "memory 512
data 9 = i8 {}
func @main(i64 %masked) -> i128 {{
entry:
 %wide = qalloc {width}
 h %wide[0]
 x %wide[{top}] ctrl %wide[0]
 x %wide[{interior}]
 %angle = xor i64 %masked, {key}
 {axis}(%angle) {location}
 %part = qalloc 128
 x %part[127] ctrl %wide[{top}]
 x %part[64] ctrl %wide[{interior}]
 x %part[63] ctrl %wide[0]
 %sample = measure %part",
        rng.random::<u8>() | (1 << 5)
    );
    if width <= 128 {
        let offset = (1_u128 << top) | 1;
        write!(
            text,
            "
 %out = qalloc {width}
 %flag = qalloc 129
 x %out[{top}]
 x %flag[128]
 apply @mix(%wide) -> %out, %flag budget 4
 %value = measure %wide
 %mixed = measure %out
 %sum = add i{width} %value, {offset}
 %expected = xor i{width} %sum, {}
 %good = icmp eq i{width} %mixed, %expected
 br %good, done, fail
done:
 ret %sample
fail:
 fault unreachable
}}
func @mix(i{width} %arg) -> i{width} {{
entry:
 %sum = add i{width} %arg, {offset}
 ret %sum
}}",
            1_u128 << top
        )
        .expect("write whole-register checks");
    } else {
        text.push_str("\n qfree %wide\n ret %sample\n}");
    }
    (text, angle.to_bits() ^ key, angle.is_finite())
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut angle_faults = 0;
    for seed in FIRST_SEED..FIRST_SEED + programs {
        let (text, masked, finite) = generate(seed);
        let counts = std::panic::catch_unwind(|| {
            let program = module(&text);
            let result = observe::run(&program, "main", &[masked], seed, BUDGET);
            let expected = if finite {
                Finish::Returned(Some(result.measurements[0].0))
            } else {
                assert!(result.measurements.is_empty());
                assert_eq!(result.reference.steps, 5);
                Finish::Faulted(Fault::BadAngle)
            };
            assert_eq!(result.reference.finish, Some(expected));
            result.counts
        })
        .unwrap_or_else(|failure| {
            eprintln!("wide register seed {seed:#018x} with argument {masked:#018x}\n{text}");
            std::panic::resume_unwind(failure)
        });
        totals.add(&counts);
        angle_faults += u64::from(!finite);
    }
    assert!(angle_faults > 0 && angle_faults < programs);
    assert_eq!(totals.states, 2 * totals.measurements + totals.programs);
    println!(
        "{} wide-register programs, {} named angle faults, {} steps, {} checkpoints, {} density entries, {} measurements",
        totals.programs, angle_faults, totals.operations, totals.states, totals.density_entries, totals.measurements
    );
}

#[test]
fn seeded_wide_registers_and_computed_angles() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "2,048 programs; cargo test -p bennett-exec --test device_reference registers_generated::seeded_wide_registers_and_computed_angles_heavy -- --ignored --exact --nocapture"]
fn seeded_wide_registers_and_computed_angles_heavy() {
    sweep(HEAVY_PROGRAMS);
}
