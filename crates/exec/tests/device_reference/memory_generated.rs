//! Seeded small programs with promotions, coherent apply, partial measurements and faults.

use std::collections::BTreeSet;

use bennett_ir::Fault;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::{BUDGET, Finish, generated::layer, module, observe};

const DEFAULT_PROGRAMS: u64 = 96;
const HEAVY_PROGRAMS: u64 = 2048;
const SEED_START: u64 = 0x41d8_2b79_43af_91b0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum End {
    Return,
    Exit,
    HeldAccess,
    Bounds,
    Alias,
}

impl End {
    fn tail(self) -> &'static str {
        match self {
            Self::Return => "ret %answer",
            Self::Exit => "call @stop(%answer)\n fault unreachable",
            Self::HeldAccess => "store i9 0x1bd, %p\n fault unreachable",
            Self::Bounds => "%bad = measure i9 [383]\n fault unreachable",
            Self::Alias => "x [%p:9] ctrl [%held:1]\n fault unreachable",
        }
    }

    fn observed(finish: Finish, tape: &[u8]) -> Self {
        match finish {
            Finish::Returned(Some(_)) if tape.len() == 2 => Self::Exit,
            Finish::Returned(Some(_)) if tape.len() == 1 => Self::Return,
            Finish::Faulted(Fault::QubitMemoryAccess) => Self::HeldAccess,
            Finish::Faulted(Fault::OutOfBounds) => Self::Bounds,
            Finish::Faulted(Fault::QubitAlias) => Self::Alias,
            _ => panic!("unexpected generated finish: {finish:?}, tape {tape:?}"),
        }
    }
}

fn generate(seed: u64) -> (String, End) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let p = rng.random_range(16..=48);
    let (first, second, plain, replacement) = rng.random::<(u8, u8, u8, u8)>();
    let end = [End::Return, End::Exit, End::HeldAccess, End::Bounds, End::Alias][rng.random_range(0..5)];
    let gates = layer(&mut rng, &["%q", "[%p:2]", "[%p:9]", "[%p:15]"]);
    let phase = rng.random_range(-2.8..=2.8);
    let access = if rng.random() {
        "%old = load i8 %addr\n store i8 %old, %addr"
    } else {
        "store i8 %value, %addr\n %old = load i8 %addr"
    };
    let finish = if matches!(end, End::Return | End::Exit) {
        "%wide = measure i19 [%p]\n %answer = load i16 %p\n effect 44"
    } else {
        ""
    };
    let tail = end.tail();
    let text = format!(
        "memory 384
data {p} = i8 {first}, i8 {second}, i8 {plain}
func @main() -> i16 {{
entry:
 %p = add i64 {p}, 0
 %held = add i64 %p, 1
 %plain = add i64 %p, 2
 %q = qalloc 1
 h %q
 x [%p:15] ctrl %q
 h [%p:2]
 x [%p:9] ctrl [%p:2]
 rz({phase:.8}) [%p:15]
 %out = qalloc 1
 %flag = qalloc 5
 {gates}
 %low = measure i5 [%plain]
 %word = zext i5 %low to i8
 apply @by_address(%q, %held, %plain, %word) -> %out, %flag budget 8
 %partial = measure i12 [%p]
 %check = load i8 %p
 store i8 {replacement}, %p
 qfree %q
 {finish}
 {tail}
}}
func @by_address(i1 %choose, i64 %held, i64 %plain, i8 %value) -> i1 {{
entry:
 %addr = select i64 %choose, %held, %plain
 {access}
 %result = trunc i8 %old to i1
 ret %result
}}
func @stop(i16 %answer) {{
entry:
 effect 45
 exit %answer
}}"
    );
    (text, end)
}

fn sweep(programs: u64) {
    let mut totals = observe::Counts::default();
    let mut ends = BTreeSet::new();
    for seed in SEED_START..SEED_START + programs {
        let (text, expected_end) = generate(seed);
        let (end, counts) = std::panic::catch_unwind(|| {
            let module = module(&text);
            let result = observe::run(&module, "main", &[], seed, BUDGET);
            let end = End::observed(result.reference.finish.expect("completed"), &result.reference.tape);
            assert_eq!(end, expected_end);
            let completes = matches!(end, End::Return | End::Exit);
            assert_eq!(result.reference.quantum.cells.len(), usize::from(!completes));
            assert_eq!(result.counts.measurements, 2 + usize::from(completes));
            (end, result.counts)
        })
        .unwrap_or_else(|failure| {
            eprintln!("memory-qubit seed {seed:#018x}\n{text}");
            std::panic::resume_unwind(failure)
        });
        ends.insert(end);
        totals.add(&counts);
    }
    assert_eq!(ends, BTreeSet::from([End::Return, End::Exit, End::HeldAccess, End::Bounds, End::Alias]));
    assert_eq!(totals.programs as u64, programs);
    assert_eq!(totals.states, 2 * totals.measurements + totals.programs);
    println!(
        "{} generated memory-qubit programs, {} operations, {} checkpoint state comparisons, {} density entries, {} measurements",
        totals.programs, totals.operations, totals.states, totals.density_entries, totals.measurements,
    );
}

#[test]
fn seeded_memory_qubit_programs() {
    sweep(DEFAULT_PROGRAMS);
}

#[test]
#[ignore = "heavy sweep"]
fn seeded_memory_qubit_programs_heavy() {
    sweep(HEAVY_PROGRAMS);
}
