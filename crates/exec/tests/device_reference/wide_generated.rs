//! Seeded whole programs using wide values and return addresses across apply boundaries.

use std::collections::BTreeSet;

use bennett_ir::Fault;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, generated::layer, module, observe};

const DEFAULT_PROGRAMS: u64 = 96;
const HEAVY_PROGRAMS: u64 = 2_048;
const FIRST_SEED: u64 = 0xde7a_9a98_61b9_f440;
const ARITHMETIC: [&str; 10] = ["add", "sub", "mul", "and", "or", "xor", "udiv", "sdiv", "urem", "srem"];
const PREDICATES: [&str; 10] = ["eq", "ne", "ult", "ule", "ugt", "uge", "slt", "sle", "sgt", "sge"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum End {
    Return,
    Exit,
    Bounds,
    ExplicitFault,
}

impl End {
    fn tail(self) -> &'static str {
        match self {
            Self::Return => "ret %answer",
            Self::Exit => "call @stop(%answer)\n fault unreachable",
            Self::Bounds => "%bad = load i128 1023\n ret %bad",
            Self::ExplicitFault => "fault unreachable",
        }
    }

    fn observed(finish: Finish, tape: &[u8]) -> Self {
        match finish {
            Finish::Returned(Some(_)) if tape == b"!" => Self::Exit,
            Finish::Returned(Some(_)) if tape.is_empty() => Self::Return,
            Finish::Faulted(Fault::OutOfBounds) => Self::Bounds,
            Finish::Faulted(Fault::Unreachable) => Self::ExplicitFault,
            _ => panic!("unexpected generated finish {finish:?}, tape {tape:?}"),
        }
    }
}

fn generate(seed: u64) -> (String, End, bool) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let index = (seed - FIRST_SEED) as usize;
    let width = 65 + index % 64;
    let mask = u128::MAX >> (128 - width);
    let sign = 1u128 << (width - 1);
    let original = rng.random::<u128>() | sign;
    let rhs = (rng.random::<u128>() & mask) | 3;
    let arithmetic = ARITHMETIC[index % ARITHMETIC.len()];
    let predicate = PREDICATES[(index / 3) % PREDICATES.len()];
    let shift = ["shl", "lshr", "ashr"][rng.random_range(0..3)];
    let distance = [0, 1, 64, width as u128 - 1, width as u128, 1 << 64][rng.random_range(0..6)];
    let depth = [0, 1, 2, 3, 4, 5, u32::MAX][index % 7];
    let end = [End::Return, End::Exit, End::Bounds, End::ExplicitFault][(index / 2) % 4];
    let applied_fault = index.is_multiple_of(5);
    let gates = layer(&mut rng, &["%q", "%out[0]", "%out[1]", "%out[2]"]);
    let pre_phase = rng.random_range(-2.8..=2.8);
    let direct_call = "call @kernel(%value, %bit)";
    let indirect_call = "call_indirect fn(i128, i1) -> i128 %fn(%value, %bit)";
    let call = if rng.random() { direct_call } else { indirect_call };
    let narrow = if width == 128 { "xor i128 %value, 0".to_string() } else { format!("trunc i128 %value to i{width}") };
    let extension = if index.is_multiple_of(2) { "zext" } else { "sext" };
    let widen =
        if width == 128 { "or i128 %loaded, 0".to_string() } else { format!("{extension} i{width} %loaded to i128") };
    let result = if applied_fault {
        "br %bit, divide, done\ndivide:\n %bad = sdiv i128 %folded, 0\n br done\ndone:\n ret %outcome"
    } else {
        "ret %outcome"
    };
    let tail = end.tail();
    let high_case = sign | 17;
    let text = format!(
        "memory 1024
data 16 = i128 {original:#x}
data 64 = i128 0xffffffffffffffffffffffffffffffff
func @main() -> i128 {{
entry:
 %q = qalloc 1
 %env = qalloc 1
 %out = qalloc 3
 %flag = qalloc 5
 %raw = load i128 16
 %ordinary = call @outer(%raw)
 h %q
 rz({pre_phase:.8}) %q
 x %env ctrl %q
 call @bridge(%q, %out, %flag, %ordinary)
 qfree %env
 %fault = measure %flag
 {gates}
 %measured = measure %out
 %m = zext i3 %measured to i128
 %f = zext i5 %fault to i128
 %memory = load i128 64
 %mixed = xor i128 %ordinary, %memory
 %first = add i128 %mixed, %m
 %answer = add i128 %first, %f
 {tail}
}}
func @outer(i128 %input) -> i128 {{
entry:
 %value = call @compute(%input, i1 1)
 ret %value
}}
func @bridge(q1 %q, q3 %out, q5 %flag, i128 %value) {{
entry:
 br live
unvisited:
 fault unreachable
live:
 apply @region(%q, %value) -> %out, %flag budget 64
 ret
}}
func @region(i1 %bit, i128 %value) -> i3 {{
entry:
 %address = retaddr {depth}
 %low = trunc i64 %address to i32
 %high = lshr i64 %address, 32
 %low_wide = zext i32 %low to i128
 %high_wide = zext i64 %high to i128
 %computed = call @compute(%value, %bit)
 %upper = lshr i128 %computed, 64
 %halves = xor i128 %computed, %upper
 %tag = xor i128 %high_wide, %low_wide
 %folded = xor i128 %halves, %tag
 %outcome = trunc i128 %folded to i3
 {result}
}}
func @compute(i128 %value, i1 %bit) -> i128 {{
entry:
 %fn = faddr @kernel
 %result = {call}
 ret %result
}}
func @kernel(i128 %value, i1 %bit) -> i128 {{
entry:
 %input = {narrow}
 %operand = select i{width} %bit, %input, {sign:#x}
 %num = {arithmetic} i{width} %operand, {rhs:#x}
 %shifted = {shift} i{width} %num, {distance:#x}
 %combined = xor i{width} %num, %shifted
 %test = icmp {predicate} i{width} %combined, {sign:#x}
 %selector = select i{width} %test, {high_case:#x}, 17
 switch i{width} %selector, 17: save(%combined, i128 1), {high_case:#x}: save(%combined, i128 {sign:#x}), default: bad
save(i{width} %word, i128 %tag):
 store i{width} %word, 64
 %loaded = load i{width} 64
 %widened = {widen}
 %address = retaddr {depth}
 %code = zext i64 %address to i128
 %sum = add i128 %widened, %tag
 %result = xor i128 %sum, %code
 ret %result
bad:
 fault unreachable
}}
func @stop(i128 %answer) {{
entry:
 effect 33
 exit %answer
}}"
    );
    (text, end, applied_fault)
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut ends = BTreeSet::new();
    for seed in FIRST_SEED..FIRST_SEED + programs {
        let (text, end, applied_fault) = generate(seed);
        let counts = std::panic::catch_unwind(|| {
            let program = module(&text);
            let result = observe::run(&program, "main", &[], seed, BUDGET);
            assert_eq!(End::observed(result.reference.finish.expect("completed"), &result.reference.tape), end);
            let [(_, probabilities), _] = result.measurements.as_slice() else {
                panic!("two measurements");
            };
            if applied_fault {
                assert_eq!(probabilities.len(), 2);
                assert!((probabilities[&0] - 0.5).abs() < 1e-11);
                assert!((probabilities[&u128::from(Fault::DivideByZero.code())] - 0.5).abs() < 1e-11);
            } else {
                assert_eq!(probabilities.len(), 1);
                assert!((probabilities[&0] - 1.0).abs() < 1e-11);
            }
            result.counts
        })
        .unwrap_or_else(|failure| {
            eprintln!("wide device seed {seed:#018x}\n{text}");
            std::panic::resume_unwind(failure)
        });
        totals.add(&counts);
        ends.insert(end);
    }
    assert_eq!(ends, BTreeSet::from([End::Return, End::Exit, End::Bounds, End::ExplicitFault]));
    assert_eq!(totals.programs as u64, programs);
    assert_eq!(totals.states, 2 * totals.measurements + totals.programs);
    println!(
        "{} generated wide device programs, {} operations, {} checkpoint state comparisons, {} density entries, {} measurements",
        totals.programs, totals.operations, totals.states, totals.density_entries, totals.measurements,
    );
}

#[test]
fn seeded_wide_device_programs() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "heavy sweep"]
fn seeded_wide_device_programs_heavy() {
    sweep(HEAVY_PROGRAMS);
}
