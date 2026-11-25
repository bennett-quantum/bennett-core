//! Every journalled step is a bijection of (machine state, history), on a clean history and on a dirty
//! one, checked by brute force at small widths: the property that lets a device keep the previous state
//! and run a step backwards.
//!
//! A machine state is everything a step reads or writes: memory, frames, live slots, stack pointer,
//! console tape, outcome, register table and amplitudes. The history is the journal. "Clean" is the
//! history the run itself wrote; "dirty" puts garbage records below it. A step appends to the history and
//! never reads below what it appends, so a dirty history must change nothing but the prefix.
//!
//! At every position a program reaches, every assignment of small values to the slots the step reads or
//! overwrites, and to two memory bytes, is stepped once. Distinct inputs must give distinct outputs
//! (injective), and stepping back must give the input exactly. The input states are arbitrary, not only
//! ones a run reaches. A rotation by a computed angle takes angles of both signs, and ones that are not
//! finite, in place of the integer values its `i64` slot would otherwise take.

use std::collections::HashMap;

use bennett_ir::{FuncId, Inst, Slot, Terminator, Type, ValidModule};

use crate::executor::{Executor, Options, Outcome};
use crate::journal::Undo;

const PROGRAM: &str = r#"
memory 256
data 16 = i8 3, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 5
data 32 = i64 @twice
data 40 = i64 0x3fe6666666666666

func @twice(i2 %x) -> i2 {
entry:
    %y = shl i2 %x, 1
    ret %y
}

func @inc(i2 %x) -> i2 {
entry:
    %y = add i2 %x, 1
    ret %y
}

; Every kind of step but measure.
func @steps(i2 %a, i2 %b) -> i2 {
entry:
    %s = add i2 %a, %b
    %d = udiv i2 %a, %b
    %c = icmp ult i2 %a, %b
    %m = select i2 %c, %a, %b
    %w = zext i2 %m to i8
    %l = load i8 16
    %n = trunc i8 %l to i2
    store i8 %w, 17
    %p = stacksave
    %buf = alloca 8
    store i8 %l, %buf
    stackrestore %p
    %f = faddr @twice
    %fa = load i64 32
    %t = call_indirect fn(i2) -> i2 %fa(%a)
    %u = call @inc(%t)
    effect %w
    %q = qalloc 2
    h %q[0]
    x %q[1] ctrl %q[0]
    %ang = load i64 40
    %neg = xor i64 %ang, 0x8000000000000000
    ry(%ang) %q[1] ctrl %q[0]
    rz(%ang) %q[0]
    p(%ang) %q[1] ctrl %q[0]
    p(%neg) %q[1] ctrl %q[0]
    rz(%neg) %q[0]
    ry(%neg) %q[1] ctrl %q[0]
    x %q[1] ctrl %q[0]
    h %q[0]
    qfree %q
    %o = qalloc 2
    %e = qalloc 5
    apply @inc(%a) -> %o, %e budget 10
    br %c, left(%s), right(%u)
left(i2 %x):
    switch i2 %x, 0: zero, 1: one, default: rest(%x)
right(i2 %y):
    %z = add i2 %y, %n
    br rest(%z)
zero:
    fault unreachable
one:
    exit i2 1
rest(i2 %v):
    ret %v
}

"#;

/// Everything a step can read or write, made comparable; amplitudes rounded so that rounding residue
/// does not make equal states differ.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    memory: Vec<u8>,
    frames: Vec<(u32, u32, usize, usize, u64)>,
    slots: Vec<u128>,
    stack_pointer: u64,
    tape: Vec<u8>,
    outcome: Option<String>,
    kinds: Vec<Undo>,
    words: Vec<u128>,
    registers: Vec<(u32, Option<u32>)>,
    /// The memory bytes that hold qubits.
    cells: Vec<(u64, usize)>,
    amplitudes: Vec<(Vec<u64>, i64, i64)>,
}

fn key(executor: &Executor) -> Key {
    let journal = executor.journal.as_ref().expect("the checks run reversibly");
    let live = executor.live_top();
    let rounded = |x: f64| (x * 1e9).round() as i64;
    Key {
        memory: executor.memory.to_vec(),
        frames: executor.frames.iter().map(|f| (f.func.0, f.block.0, f.inst, f.base, f.entry_stack_pointer)).collect(),
        slots: executor.slots[..live].to_vec(),
        stack_pointer: executor.stack_pointer,
        tape: executor.tape.clone(),
        outcome: executor.outcome.map(|outcome| format!("{outcome:?}")),
        kinds: (0..journal.records()).map(|index| journal.kind_at(index)).collect(),
        words: journal.words().to_vec(),
        registers: executor.quantum.registers().iter().map(|r| (r.generation, r.width.map(|w| w.bits()))).collect(),
        cells: executor.quantum.cells().iter().map(|(&address, &column)| (address, column)).collect(),
        amplitudes: executor
            .quantum
            .amplitudes()
            .iter()
            .map(|(config, amplitude)| (config.to_vec(), rounded(amplitude.re), rounded(amplitude.im)))
            .collect(),
    }
}

/// Garbage below the run's own history: records of every kind with words that describe nothing.
fn make_dirty(executor: &mut Executor) {
    let journal = executor.journal.as_mut().expect("the checks run reversibly");
    for (kind, word) in [(Undo::Jump, 0xdead), (Undo::Store, 0xbeef), (Undo::Call, 7), (Undo::Return, 1 << 100)] {
        journal.push_word(word);
        journal.push_word(word ^ 1);
        journal.push(kind);
    }
}

/// The small values a slot of `ty` takes: every value up to 2 bits, a few otherwise. Addresses include
/// one outside memory, so faulting steps are covered too.
fn values(ty: Type) -> Vec<u128> {
    match ty {
        Type::Int(width) if width.bits() <= 2 => (0..=width.mask()).collect(),
        Type::Int(width) if width.bits() == 64 => vec![16, 24, 200, u128::from(u64::MAX)],
        Type::Int(width) => vec![0, 1, 3, width.mask()],
        Type::Qreg(_) => Vec::new(),
    }
}

/// Every assignment of `choices`, one value per entry.
fn assignments(choices: &[Vec<u128>]) -> Vec<Vec<u128>> {
    choices.iter().fold(vec![Vec::new()], |partial, options| {
        partial
            .iter()
            .flat_map(|prefix| {
                options.iter().map(move |&value| {
                    let mut next = prefix.clone();
                    next.push(value);
                    next
                })
            })
            .collect()
    })
}

/// A place a variation writes: a slot of the frame at a depth, or a byte of memory.
#[derive(Clone, Copy, Debug)]
enum Place {
    Slot { frame: usize, slot: Slot },
    Byte(usize),
}

/// The angles a computed rotation's slot takes: finite ones of both signs and the two kinds that fault
/// `bad_angle`. The `i64` defaults of `values`, read as binary64, are subnormal: a rotation by them is the
/// identity to within the comparison's tolerance, so a wrong inverse would not show.
const ANGLES: [f64; 4] = [0.7, -1.89, f64::INFINITY, f64::NAN];

const BYTES: [usize; 2] = [16, 24];
const BYTE_VALUES: [u128; 2] = [0, 0x5a];

struct Fixture<'m> {
    module: &'m ValidModule,
    entry: FuncId,
    args: Vec<u64>,
    dirty: bool,
}

impl<'m> Fixture<'m> {
    /// A fresh reversible run, on a dirty history if asked, advanced `steps` steps.
    fn at(&self, steps: usize) -> Executor<'m> {
        let options = Options { reversible: true, budget: Some(10_000), ..Options::default() };
        let mut executor = Executor::new(self.module, self.entry, &self.args, options).expect("starts");
        if self.dirty {
            make_dirty(&mut executor);
        }
        for _ in 0..steps {
            assert_eq!(executor.step(), None, "the fixture reaches its position");
        }
        executor
    }
}

fn assign(executor: &mut Executor, places: &[Place], values: &[u128]) {
    for (&place, &value) in places.iter().zip(values) {
        match place {
            Place::Slot { frame, slot } => {
                let index = executor.frames[frame].base + slot.index();
                executor.slots[index] = value;
            }
            // A byte that holds qubits is 0 in memory; its value is the cell's, which changes instead.
            Place::Byte(address) => match executor.quantum.cell(address as u64) {
                Some(column) => executor.quantum.xor_register(column, value as u64),
                None => executor.memory.write(address, &[value as u8]),
            },
        }
    }
}

/// The slots the next step reads or overwrites, with their value choices: its operands, its result,
/// the parameters of every block it may branch to, and for a return the caller's result slot.
fn places_of_step(executor: &Executor) -> Vec<(Place, Vec<u128>)> {
    let depth = executor.frames.len() - 1;
    let frame = *executor.top();
    let function = executor.function(frame.func);
    let block = function.block(frame.block);
    let mut slots = Vec::new();
    let angle = match block.insts.get(frame.inst) {
        Some(Inst::RotateBy { angle, .. }) => Some(*angle),
        _ => None,
    };
    match block.insts.get(frame.inst) {
        Some(inst) => {
            inst.visit_uses(|slot| slots.push((depth, slot)));
            slots.extend(inst.result().map(|slot| (depth, slot)));
        }
        None => {
            block.term.visit_uses(|slot| slots.push((depth, slot)));
            block.term.visit_targets(|target| {
                slots.extend(function.block(target.block).params.iter().map(|param| (depth, param.slot)));
            });
            if matches!(block.term, Terminator::Return(_)) && depth > 0 {
                let caller = executor.frames[depth - 1];
                let call = &executor.function(caller.func).block(caller.block).insts[caller.inst];
                slots.extend(call.result().map(|slot| (depth - 1, slot)));
            }
        }
    }
    slots.sort_by_key(|&(depth, slot)| (depth, slot.0));
    slots.dedup_by_key(|&mut (depth, slot)| (depth, slot.0));
    let mut places: Vec<(Place, Vec<u128>)> = slots
        .into_iter()
        .map(|(frame, slot)| {
            let func = executor.frames[frame].func;
            let choices = if frame == depth && Some(slot) == angle {
                ANGLES.iter().map(|angle| u128::from(angle.to_bits())).collect()
            } else {
                values(executor.module.slot_type(func, slot))
            };
            (Place::Slot { frame, slot }, choices)
        })
        .filter(|(_, choices)| !choices.is_empty())
        .collect();
    places.extend(BYTES.iter().map(|&address| (Place::Byte(address), BYTE_VALUES.to_vec())));
    places
}

/// The position each step of a run starts at, and how many steps reach it first.
fn positions(fixture: &Fixture) -> Vec<(usize, (u32, u32, usize, usize))> {
    let mut executor = fixture.at(0);
    let mut seen = HashMap::new();
    let mut order = Vec::new();
    for step in 0.. {
        if executor.outcome.is_some() {
            break;
        }
        let frame = *executor.top();
        let position = (frame.func.0, frame.block.0, frame.inst, executor.frames.len());
        if seen.insert(position, step).is_none() {
            order.push((step, position));
        }
        executor.step();
    }
    order
}

fn next_inst<'m>(executor: &Executor<'m>) -> Option<&'m Inst> {
    let frame = *executor.top();
    executor.function(frame.func).block(frame.block).insts.get(frame.inst)
}

/// What `check_single_steps` covered: single steps undone, faulting steps, and among them the
/// rotations by a computed angle (`Inst::RotateBy`), split into those undone, those that turned a byte into
/// qubits, and those that faulted `bad_angle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    checked: usize,
    faults: usize,
    rotations: usize,
    promoting_rotations: usize,
    bad_angles: usize,
}

impl std::ops::Add for Counts {
    type Output = Counts;
    fn add(self, other: Counts) -> Counts {
        Counts {
            checked: self.checked + other.checked,
            faults: self.faults + other.faults,
            rotations: self.rotations + other.rotations,
            promoting_rotations: self.promoting_rotations + other.promoting_rotations,
            bad_angles: self.bad_angles + other.bad_angles,
        }
    }
}

/// Single steps: injective, and undone exactly, from every assignment at every reached position.
fn check_single_steps(fixture: &Fixture) -> Counts {
    let mut counts = Counts::default();
    for (steps, position) in positions(fixture) {
        let probe = fixture.at(steps);
        if matches!(next_inst(&probe), Some(Inst::Measure { .. })) {
            continue;
        }
        let (places, choices): (Vec<Place>, Vec<Vec<u128>>) = places_of_step(&probe).into_iter().unzip();
        // For a computed rotation, the index of its angle among the varied places.
        let angle = match next_inst(&probe) {
            Some(Inst::RotateBy { angle, .. }) => {
                places.iter().position(|place| matches!(place, Place::Slot { slot, .. } if slot == angle))
            }
            _ => None,
        };
        let mut outputs = HashMap::new();
        for assignment in assignments(&choices) {
            let mut executor = fixture.at(steps);
            assign(&mut executor, &places, &assignment);
            let before = key(&executor);
            let at_step = executor.snapshot();
            executor.step();
            let after = key(&executor);
            assert!(
                after.kinds.starts_with(&before.kinds) && after.words.starts_with(&before.words),
                "{position:?} {assignment:?}: the step changed the history below what it appended"
            );
            // A fault, or a stop of the simulator, changes nothing but the outcome.
            let faulted = matches!(executor.outcome, Some(Outcome::Faulted(_) | Outcome::Stopped(_)));
            if let Some(other) = outputs.insert(after.clone(), assignment.clone()) {
                panic!("{position:?}: assignments {other:?} and {assignment:?} step to the same state");
            }
            if faulted {
                // A faulting step changes nothing but the outcome; returning to running is its inverse.
                let mut unchanged = after;
                unchanged.outcome = None;
                assert_eq!(unchanged, before, "{position:?} {assignment:?}: a fault changed the state");
                counts.faults += 1;
                // The angle is read before anything else, so an angle that is not finite faults
                // `bad_angle` whatever the qubits' addresses are.
                let bad_angle = executor.outcome == Some(Outcome::Faulted(bennett_ir::Fault::BadAngle));
                if let Some(index) = angle {
                    let finite = f64::from_bits(assignment[index] as u64).is_finite();
                    assert_eq!(bad_angle, !finite, "{position:?} {assignment:?}");
                    counts.bad_angles += usize::from(bad_angle);
                }
                continue;
            }
            if angle.is_some() {
                counts.rotations += 1;
                counts.promoting_rotations += usize::from(after.kinds.last() == Some(&Undo::GateAtPromoted));
            }
            assert_eq!(executor.step_back(), Ok(true), "{position:?} {assignment:?}");
            assert_eq!(key(&executor).kinds, before.kinds);
            let mut back = executor.snapshot();
            back.classical.steps = at_step.classical.steps;
            assert_eq!(back.classical, at_step.classical, "{position:?} {assignment:?}: not undone exactly");
            let distance = back.quantum.distance(&at_step.quantum);
            assert!(distance.is_some_and(|d| d <= 1e-12), "{position:?} {assignment:?}: amplitudes {distance:?}");
            assert_eq!(key(&executor).words, before.words, "{position:?} {assignment:?}");
            counts.checked += 1;
        }
    }
    counts
}

fn fixtures(module: &ValidModule) -> Vec<Fixture<'_>> {
    let mut fixtures = Vec::new();
    for dirty in [false, true] {
        // Down each arm of @steps (a switch default, the right arm, an exit, a fault, a division by zero).
        for args in [[1, 2], [2, 1], [0, 1], [1, 3], [3, 0]] {
            let entry = module.function_named("steps").expect("entry");
            fixtures.push(Fixture { module, entry, args: args.to_vec(), dirty });
        }
    }
    fixtures
}

#[test]
fn every_journalled_step_is_a_bijection_on_clean_and_dirty_history() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let counts = fixtures(&module).iter().map(check_single_steps).fold(Counts::default(), |a, b| a + b);
    // The counts pin the coverage: a change to the program or the value sets shows here.
    let expected = Counts { checked: 33_352, faults: 3_336, rotations: 384, promoting_rotations: 0, bad_angles: 384 };
    assert_eq!(counts, expected, "single steps checked, faulting ones, computed rotations");
}

#[test]
fn the_fixture_reaches_every_kind_of_step() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let mut kinds = std::collections::BTreeSet::new();
    let mut outcomes = std::collections::BTreeSet::new();
    for fixture in fixtures(&module) {
        let mut executor = fixture.at(0);
        let outcome = executor.run();
        outcomes.insert(format!("{outcome:?}"));
        let journal = executor.journal.as_ref().expect("reversible");
        kinds.extend((0..journal.records()).map(|index| journal.kind_at(index)));
        assert!(!matches!(outcome, Outcome::Stopped(_)), "{outcome:?}");
    }
    for kind in [
        Undo::Slot,
        Undo::Store,
        Undo::Alloca,
        Undo::StackRestore,
        Undo::Effect,
        Undo::Call,
        Undo::Jump,
        Undo::Return,
        Undo::Finish,
        Undo::Gate,
        Undo::Qalloc,
        Undo::Qfree,
        Undo::Apply,
    ] {
        assert!(kinds.contains(&kind), "no {kind:?} step in the fixtures");
    }
    assert!(outcomes.len() >= 3, "returns, an exit and a fault: {outcomes:?}");
}

/// Qubits in memory (`docs/DESIGN.md`, "Qubits in memory"): gates at addresses that promote bytes to
/// cells, at an address that depends on an argument (in bounds, out of bounds, aliasing another qubit),
/// coherent regions beside cells, gates after them that promote further bytes, and then, as `%c` chooses,
/// ordinary code touching a cell, which faults `qubit_memory_access` whatever the cell holds: a load, a store
/// over it, a `setjmp` record written over it (after one written to plain memory), and an applied function
/// reading it (a region fault, the simulator's stop). `assign` changes a cell's value where a varied byte
/// holds qubits.
const CELLS: &str = r#"
memory 256
data 16 = i8 3, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 5
data 48 = i8 7
data 56 = i64 0x3fe6666666666666

func @peek(i2 %x) -> i8 {
entry:
    %v = load i8 48
    %e = zext i2 %x to i8
    %t = xor i8 %v, %e
    ret %t
}

func @peek_cell(i2 %x) -> i8 {
entry:
    %v = load i8 16
    ret %v
}

func @cells(i2 %a, i2 %b, i2 %c) -> i2 {
entry:
    %q = qalloc 2
    %o = qalloc 8
    %e = qalloc 5
    %p = zext i2 %a to i64
    %addr = add i64 %p, 16
    %far = mul i64 %p, 120
    %near = add i64 %p, 32
    %ang = load i64 56
    %neg = xor i64 %ang, 0x8000000000000000
    h [16:1]
    x [%addr:0] ctrl [16:1]
    x [%addr:1] ctrl [16:1]
    x %q[0] ctrl [16:1]
    s [24:3] ctrl %q[0]
    t [%far:2]
    rx(%ang) [%near:6] ctrl [16:1]
    apply @peek(%b) -> %o, %e budget 20
    apply @peek(%b) -> %o, %e budget 20
    rx(%neg) [%near:6] ctrl [16:1]
    tdg [%far:2]
    sdg [24:3] ctrl %q[0]
    x %q[0] ctrl [16:1]
    x [%addr:1] ctrl [16:1]
    x [%addr:0] ctrl [16:1]
    h [16:1]
    x [24:5]
    h [40:0]
    x [40:0] ctrl [24:5]
    switch i2 %c, 0: load, 1: store, 2: jump, default: applied
load:
    %l = load i16 16
    %n = trunc i16 %l to i2
    ret %n
store:
    store i16 3, 23
    ret 0
jump:
    %k = setjmp 64
    %j = setjmp 16
    ret 1
applied:
    apply @peek_cell(%b) -> %o, %e budget 20
    ret 2
}
"#;

fn cell_fixtures(module: &ValidModule) -> Vec<Fixture<'_>> {
    let entry = module.function_named("cells").expect("entry");
    let mut fixtures = Vec::new();
    for dirty in [false, true] {
        // Each way of touching a cell, after gates in bounds; in bounds at another address; `[%addr:1]`
        // aliasing `[16:1]` (a = 0); out of bounds (a = 3). The last three fault before `%c` is read.
        let args = (0..4).map(|c| [1, 2, c]).chain([[2, 1, 0], [0, 1, 0], [3, 0, 0]]);
        for args in args {
            fixtures.push(Fixture { module, entry, args: args.to_vec(), dirty });
        }
    }
    fixtures
}

#[test]
fn every_step_at_memory_qubits_is_a_bijection_on_clean_and_dirty_history() {
    let module = bennett_ir::load(CELLS).expect("valid");
    let fixtures = cell_fixtures(&module);
    let counts = fixtures.iter().map(check_single_steps).fold(Counts::default(), |a, b| a + b);
    let mut kinds = std::collections::BTreeSet::new();
    let mut outcomes = std::collections::BTreeSet::new();
    for fixture in &fixtures {
        let mut executor = fixture.at(0);
        outcomes.insert(format!("{:?}", executor.run()));
        let journal = executor.journal.as_ref().expect("reversible");
        kinds.extend((0..journal.records()).map(|index| journal.kind_at(index)));
    }
    assert!(kinds.contains(&Undo::GateAt) && kinds.contains(&Undo::GateAtPromoted) && kinds.contains(&Undo::Setjmp));
    assert!(outcomes.iter().any(|o| o.contains("QubitAlias")) && outcomes.iter().any(|o| o.contains("OutOfBounds")));
    // Each way of touching a cell, after the gates in bounds (a = 1, b = 2): a load, a store and a `setjmp`
    // fault alike; an applied function reading a cell faults for both values of the cell's qubit [16:1]
    // that remain (x [24:5] and h [40:0] leave two configurations), which strict mode stops at.
    let peek_cell = module.function_named("peek_cell").expect("peek_cell");
    for c in 0..4 {
        let fixture = Fixture { module: &module, entry: fixtures[0].entry, args: vec![1, 2, c], dirty: false };
        let expected = match c {
            0..=2 => Outcome::Faulted(bennett_ir::Fault::QubitMemoryAccess),
            _ => Outcome::Stopped(crate::RunError::RegionFault {
                callee: peek_cell,
                fault: bennett_ir::Fault::QubitMemoryAccess,
                faulting: 2,
                configurations: 2,
            }),
        };
        assert_eq!(fixture.at(0).run(), expected, "c = {c}");
    }
    println!("memory qubits: {counts:?}; outcomes {outcomes:?}");
    let expected = Counts { checked: 7_000, faults: 1_312, rotations: 480, promoting_rotations: 160, bad_angles: 640 };
    assert_eq!(counts, expected, "single steps, faulting ones, computed rotations");
}
