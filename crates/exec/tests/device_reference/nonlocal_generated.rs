//! Seeded runs of the same jump routine on outer inputs and on superposed apply inputs.

use std::fmt::Write as _;

use bennett_ir::Fault;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, module, observe};

const FIRST_SEED: u64 = 0x857b_2bc9_8a1e_a210;
const DEFAULT_PROGRAMS: u64 = 96;
const HEAVY_PROGRAMS: u64 = 2_048;

#[derive(Clone, Copy)]
enum Route {
    Direct,
    Unwind,
    BadRecord,
    Bounds,
    Held,
}

impl Route {
    const ALL: [Self; 5] = [Self::Direct, Self::Unwind, Self::BadRecord, Self::Bounds, Self::Held];

    fn address(self, record: u64, boundary: u64) -> u64 {
        match self {
            Self::Direct | Self::Unwind => record,
            Self::BadRecord => record + 40,
            Self::Bounds => boundary,
            Self::Held => 992,
        }
    }

    fn finish(self, direct: u8, unwound: u8) -> Finish {
        match self {
            Self::Direct => Finish::Returned(Some(u128::from(direct))),
            Self::Unwind => Finish::Returned(Some(u128::from(unwound))),
            Self::BadRecord => Finish::Faulted(Fault::BadLongjmp),
            Self::Bounds => Finish::Faulted(Fault::OutOfBounds),
            Self::Held => Finish::Faulted(Fault::QubitMemoryAccess),
        }
    }
}

fn check_seed(seed: u64, outer: Route, applied: Route) -> observe::Counts {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let record = rng.random_range(16..=80);
    let before = rng.random_range(1..=39);
    let after = rng.random_range(1..=39);
    let direct = rng.random_range(1..=127);
    let value = if rng.random_ratio(1, 4) { 0 } else { rng.random::<i32>() | 128 };
    let angle = rng.random_range(0.25_f64..=2.8);
    let phase = if rng.random() { "s" } else { "t" };
    let boundary = if rng.random() { u64::MAX } else { rng.random_range(993..=1023) };
    let outer_address = outer.address(record, boundary);
    let applied_address = applied.address(record, boundary);
    let take_jump = u8::from(!matches!(outer, Route::Direct));
    let hops = rng.random_range(1..=4);
    let mut next = String::from("leaf");
    let mut calls = String::new();
    for index in 1..hops {
        let name = format!("hop{index}");
        let call = if rng.random() {
            format!("call @{next}(%buf, %value, %scratch)")
        } else {
            format!("%fn = faddr @{next}\n call_indirect fn(i64, i32, i64) %fn(%buf, %value, %scratch)")
        };
        writeln!(
            calls,
            "func @{name}(i64 %buf, i32 %value, i64 %scratch) {{
entry:
 {call}
 fault unreachable
}}"
        )
        .expect("write generated call");
        next = name;
    }
    let text = format!(
        "memory 1024
func @main() -> i8 {{
entry:
 %q = qalloc 1
 %env = qalloc 1
 %out = qalloc 8
 %flag = qalloc 5
 ry({angle:.8}) %q
 {phase} %q
 x %env ctrl %q
 x [1023] ctrl %q
 apply @worker(%q, i64 {applied_address}, i32 {value}) -> %out, %flag budget 40
 qfree %env
 %observed = measure %out
 %code = measure %flag
 %dirty = load i1 0
 br %dirty, dirty, clean
dirty:
 fault unreachable
clean:
 %answer = call @worker(i1 {take_jump}, i64 {outer_address}, i32 {value})
 ret %answer
}}
func @worker(i1 %take, i64 %destination, i32 %value) -> i8 {{
entry:
 %before = alloca {before}
 %r = setjmp {record}
 %marker = load i1 0
 br %marker, resumed, choose
choose:
 br %take, travel, direct
direct:
 %initial = icmp eq i32 %r, 0
 %direct = select i8 %initial, {direct}, 0
 ret %direct
travel:
 store i1 1, 0
 %after = alloca {after}
 call @{next}(%destination, %value, %after)
 fault unreachable
resumed:
 %saved = stacksave
 %same = icmp eq i64 %saved, %before
 %byte = trunc i32 %r to i8
 br %same, done, invalid
done:
 ret %byte
invalid:
 fault unreachable
}}
func @leaf(i64 %buf, i32 %value, i64 %scratch) {{
entry:
 store i32 %value, %scratch
 longjmp %buf, %value
}}
{calls}"
    );

    std::panic::catch_unwind(|| {
        let program = module(&text);
        let result = observe::run(&program, "main", &[], seed, BUDGET);
        let unwound = value as u8;
        assert_eq!(result.reference.finish, Some(outer.finish(direct, unwound)));
        assert_eq!(result.reference.memory[0], take_jump);
        let [(outcome, distribution), (code, _)] = result.measurements.as_slice() else {
            panic!("two observations of the applied result");
        };
        let (applied_value, fault_code) = match applied.finish(direct, unwound) {
            Finish::Returned(Some(value)) => (value, 0),
            Finish::Faulted(fault) => (0, u128::from(fault.code())),
            _ => unreachable!(),
        };
        let (sin, cos) = (angle / 2.0).sin_cos();
        assert_eq!(distribution.len(), 2);
        assert!((distribution[&u128::from(direct)] - cos * cos).abs() < 5e-9);
        assert!((distribution[&applied_value] - sin * sin).abs() < 5e-9);
        assert_eq!(*code, if *outcome == u128::from(direct) { 0 } else { fault_code });
        result.counts
    })
    .unwrap_or_else(|failure| {
        eprintln!("non-local jump seed {seed:#018x}\n{text}");
        std::panic::resume_unwind(failure)
    })
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut outer_counts = [0; Route::ALL.len()];
    let mut applied_counts = [0; Route::ALL.len() - 1];
    for index in 0..programs {
        let outer = index as usize % outer_counts.len();
        let applied = (index as usize / outer_counts.len()) % applied_counts.len();
        totals.add(&check_seed(FIRST_SEED + index, Route::ALL[outer], Route::ALL[applied + 1]));
        outer_counts[outer] += 1;
        applied_counts[applied] += 1;
    }
    assert!(outer_counts.iter().chain(&applied_counts).all(|&count| count > 0));
    assert_eq!(totals.programs as u64, programs);
    assert_eq!(totals.measurements, 2 * totals.programs);
    println!(
        "{} generated non-local jump programs, {} operations, {} checkpoint state comparisons, {} density entries, {} measurements",
        totals.programs, totals.operations, totals.states, totals.density_entries, totals.measurements,
    );
    println!("outer (direct, unwind, bad record, bounds, held): {outer_counts:?}");
    println!("applied input 1 (unwind, bad record, bounds, held): {applied_counts:?}");
}

#[test]
fn seeded_nonlocal_device_programs() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "heavy sweep"]
fn seeded_nonlocal_device_programs_heavy() {
    sweep(HEAVY_PROGRAMS);
}
