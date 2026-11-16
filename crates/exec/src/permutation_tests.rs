//! The first experiment of the journalled machine (`docs/DESIGN.md`): every journalled step is a bijection
//! of (machine state, history), on a clean history and on a dirty one, checked by brute force at small
//! widths.
//!
//! A machine state is everything a step reads or writes: memory, frames, live slots, stack pointer,
//! console tape, outcome, register table and amplitudes. The history is the journal. "Clean" is the
//! history the run itself wrote; "dirty" puts garbage records below it, as code that left its history
//! behind would. A step appends to the history and never reads below what it appends, so a dirty history
//! must change nothing but the prefix.
//!
//! Single steps: at every position a program reaches, every assignment of small values to the slots the
//! step reads or overwrites, and to two memory bytes, is stepped once. Distinct inputs must give distinct
//! outputs (injective), and stepping back must give the input exactly. The input states are arbitrary,
//! not only ones a run reaches.
//!
//! A rewind is a bijection only of states its history describes: on others it would discard what the
//! history does not record. So it is checked as the composite from its mark: every assignment at the
//! mark is run to the rewind and through it. The result must be the state at the mark with the program
//! counter after the rewind, the handle set and the kept values XORed into their registers; distinct
//! starts must give distinct results; and stepping back must give the state at the rewind exactly.

use std::collections::HashMap;

use bennett_ir::{FuncId, Inst, Operand, Slot, Terminator, Type, ValidModule};

use crate::executor::{Executor, Options, Outcome};
use crate::journal::Undo;

const PROGRAM: &str = r#"
memory 256
data 16 = i8 3, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 5
data 32 = i64 @twice

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

; Every kind of step but measure and rewind, which are checked apart.
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
    x %q[1] ctrl %q[0]
    h %q[0]
    qfree %q
    %o = qalloc 2
    %e = qalloc 5
    apply @inc(%a) -> %o, %e budget 10
    %h = mark
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

; Reads memory and its arguments, branches, writes memory and the stack, prints, and rewinds a nested
; mark of its own, so that the caller's rewind runs that region again.
func @mix(i2 %a, i2 %b) -> i2 {
entry:
    %g = mark
    %l = load i8 24
    %k = trunc i8 %l to i2
    %s = add i2 %a, %k
    %c = icmp eq i2 %b, 2
    br %c, odd, even
odd:
    %t = xor i2 %s, %b
    br done(%t)
even:
    %buf = alloca 8
    store i2 %s, %buf
    %r = load i2 %buf
    br done(%r)
done(i2 %v):
    %w = zext i2 %v to i8
    store i8 %w, 24
    effect %w
    rewind %g
    %x = mul i2 %a, 3
    ret %x
}

func @region(i2 %a, i2 %b) -> i2 {
entry:
    %r = qalloc 2
    %x = add i2 %a, 1
    %h = mark
    %y = call @mix(%a, %b)
    %z = add i2 %y, %x
    rewind %h keep (%y -> %r, %z -> %r)
    ret %x
}
"#;

/// Everything a step can read or write, made comparable; amplitudes rounded so that rounding residue
/// does not make equal states differ.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    memory: Vec<u8>,
    /// How the memory is held: the overlay over the shared base (`memory.rs`). Normal, so it follows from
    /// `memory` once the base is fixed; kept to check that a step restores the representation too.
    overlay: Vec<(usize, u8)>,
    frames: Vec<(u32, u32, usize, usize, u64)>,
    slots: Vec<u128>,
    stack_pointer: u64,
    tape: Vec<u8>,
    outcome: Option<String>,
    kinds: Vec<Undo>,
    words: Vec<u128>,
    marks: Vec<(usize, u64)>,
    reads: Vec<(usize, usize, u32)>,
    registers: Vec<(u32, Option<u32>)>,
    /// The memory bytes that hold qubits, and the journal's notes of cells that steps touched (without
    /// whether the access split the world, which is how the state is held, as for `reads`).
    cells: Vec<(u64, usize)>,
    cell_entries: Vec<(usize, usize, u8)>,
    amplitudes: Vec<(Vec<u64>, i64, i64)>,
}

fn key(executor: &Executor) -> Key {
    let journal = executor.journal.as_ref().expect("the checks run reversibly");
    let live = executor.live_top();
    let rounded = |x: f64| (x * 1e9).round() as i64;
    assert!(executor.memory.is_normal(), "an overlay entry equal to the base");
    Key {
        memory: executor.memory.to_vec(),
        overlay: executor.memory.overlay(),
        frames: executor.frames.iter().map(|f| (f.func.0, f.block.0, f.inst, f.base, f.entry_stack_pointer)).collect(),
        slots: executor.slots[..live].to_vec(),
        stack_pointer: executor.stack_pointer,
        tape: executor.tape.clone(),
        outcome: executor.outcome.map(|outcome| format!("{outcome:?}")),
        kinds: (0..journal.records()).map(|index| journal.kind_at(index)).collect(),
        words: journal.words().to_vec(),
        marks: journal.marks().iter().map(|entry| (entry.position, entry.allocations)).collect(),
        reads: journal.reads_from(0).iter().map(|read| (read.record, read.column, read.slot.0)).collect(),
        registers: executor.quantum.registers().iter().map(|r| (r.generation, r.width.map(|w| w.bits()))).collect(),
        cells: executor.quantum.cells().iter().map(|(&address, &column)| (address, column)).collect(),
        cell_entries: journal.cell_entries().iter().map(|e| (e.record, e.column, e.value)).collect(),
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
    for (kind, word) in [(Undo::Jump, 0xdead), (Undo::Store, 0xbeef), (Undo::Mark, 7), (Undo::Return, 1 << 100)] {
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

const BYTES: [usize; 2] = [16, 24];
const BYTE_VALUES: [u128; 2] = [0, 0x5a];

struct Fixture<'m> {
    module: &'m ValidModule,
    entry: FuncId,
    args: Vec<u64>,
    dirty: bool,
    /// Whether memory is held as an overlay over a shared base (as with several worlds), and the base's
    /// byte at each of `BYTES` under it; `None`: one world owning its memory.
    shared: Option<u8>,
}

impl<'m> Fixture<'m> {
    /// A fresh reversible run, on a dirty history if asked, advanced `steps` steps.
    fn at(&self, steps: usize) -> Executor<'m> {
        let options = Options { reversible: true, budget: Some(10_000), ..Options::default() };
        let mut executor = Executor::new(self.module, self.entry, &self.args, options).expect("starts");
        if let Some(byte) = self.shared {
            for address in BYTES {
                executor.memory.write(address, &[byte]);
            }
            executor.memory.pin();
        }
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
            (Place::Slot { frame, slot }, values(executor.module.slot_type(func, slot)))
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

/// Single steps: injective, and undone exactly, from every assignment at every reached position. Returns
/// the steps checked and how many of them faulted.
fn check_single_steps(fixture: &Fixture) -> (usize, usize) {
    let (mut checked, mut faults) = (0, 0);
    for (steps, position) in positions(fixture) {
        let probe = fixture.at(steps);
        if matches!(next_inst(&probe), Some(Inst::Measure { .. } | Inst::Rewind { .. })) {
            continue;
        }
        let (places, choices): (Vec<Place>, Vec<Vec<u128>>) = places_of_step(&probe).into_iter().unzip();
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
            let faulted = matches!(executor.outcome, Some(Outcome::Faulted(_)));
            if let Some(other) = outputs.insert(after.clone(), assignment.clone()) {
                panic!("{position:?}: assignments {other:?} and {assignment:?} step to the same state");
            }
            if faulted {
                // A faulting step changes nothing but the outcome; returning to running is its inverse.
                let mut unchanged = after;
                unchanged.outcome = None;
                assert_eq!(unchanged, before, "{position:?} {assignment:?}: a fault changed the state");
                faults += 1;
                continue;
            }
            assert_eq!(executor.step_back(), Ok(true), "{position:?} {assignment:?}");
            assert_eq!(key(&executor).kinds, before.kinds);
            let mut back = executor.snapshot();
            back.classical.steps = at_step.classical.steps;
            assert_eq!(back.classical, at_step.classical, "{position:?} {assignment:?}: not undone exactly");
            let distance = back.quantum.distance(&at_step.quantum);
            assert!(distance.is_some_and(|d| d <= 1e-12), "{position:?} {assignment:?}: amplitudes {distance:?}");
            assert_eq!(key(&executor).words, before.words, "{position:?} {assignment:?}");
            checked += 1;
        }
    }
    (checked, faults)
}

/// Whether the run from a `mark` (the next step) reaches a rewind of that mark in the same frame.
fn reaches_its_rewind(mut executor: Executor, handle: Slot) -> bool {
    let depth = executor.frames.len();
    executor.step();
    let value = executor.slot(handle);
    while executor.outcome.is_none() {
        if executor.frames.len() == depth
            && let Some(Inst::Rewind { mark, .. }) = next_inst(&executor)
            && executor.slot(*mark) == value
        {
            return true;
        }
        executor.step();
    }
    false
}

/// Rewinds, as the composite from their mark: from every assignment at the mark, run to the rewind and
/// through it.
fn check_rewinds(fixture: &Fixture) -> usize {
    let mut checked = 0;
    for (steps, position) in positions(fixture) {
        let probe = fixture.at(steps);
        let Some(Inst::Mark { dst: handle, .. }) = next_inst(&probe) else { continue };
        if !reaches_its_rewind(fixture.at(steps), *handle) {
            continue;
        }
        let depth = probe.frames.len();
        let func = probe.top().func;
        let function = probe.function(func);
        // Varied: the parameters (what the region reads), the handle's old value, two memory bytes and
        // the register kept into. Every other slot starts as garbage, which the rewind must restore.
        let mut places = Vec::new();
        let mut choices = Vec::new();
        let mut garbage = Vec::new();
        for slot in (0..function.slot_count()).map(|index| Slot(index as u32)) {
            let options = values(probe.module.slot_type(func, slot));
            let Some(&last) = options.last() else { continue };
            let place = Place::Slot { frame: depth - 1, slot };
            if slot == *handle || function.params.iter().any(|param| param.slot == slot) {
                places.push(place);
                choices.push(options);
            } else {
                garbage.push((place, last));
            }
        }
        for address in BYTES {
            places.push(Place::Byte(address));
            choices.push(BYTE_VALUES.to_vec());
        }
        let registers: Vec<usize> = (0..probe.quantum.registers().len()).collect();
        choices.push(if registers.is_empty() { vec![0] } else { (0..4).collect() });
        let mut outputs = HashMap::new();
        for assignment in assignments(&choices) {
            let mut executor = fixture.at(steps);
            let (start, register_value) = assignment.split_at(places.len());
            let (garbage_places, garbage_values): (Vec<Place>, Vec<u128>) = garbage.iter().copied().unzip();
            assign(&mut executor, &garbage_places, &garbage_values);
            assign(&mut executor, &places, start);
            if let Some(&column) = registers.first() {
                executor.quantum.xor_register(column, register_value[0] as u64);
            }
            let at_mark = key(&executor);
            executor.step();
            let mark_value = executor.slot(*handle);
            // Run to this mark's rewind in this frame; a start that faults first has no rewind to check.
            let rewind = loop {
                if executor.outcome.is_some() {
                    break None;
                }
                if executor.frames.len() == depth
                    && let Some(Inst::Rewind { mark, keep }) = next_inst(&executor)
                    && executor.slot(*mark) == mark_value
                {
                    break Some(keep);
                }
                executor.step();
            };
            let Some(keep) = rewind else { continue };
            let at_rewind = executor.snapshot();
            let at_rewind_key = key(&executor);
            let kept: Vec<(usize, u64)> = keep
                .iter()
                .map(|pair| {
                    let bennett_ir::KeepTarget::Register(register) = pair.target else {
                        unreachable!("the fixtures keep into registers")
                    };
                    let column = executor.quantum.resolve(executor.handle(register)).expect("live");
                    let value = match pair.value {
                        Operand::Slot(slot) => executor.slot(slot),
                        Operand::Const(value) => value.value(),
                    };
                    (column, value as u64)
                })
                .collect();
            assert_eq!(executor.step(), None, "{position:?} {assignment:?}: the rewind runs");
            let after = key(&executor);
            // The state at the mark, moved past the rewind, with the handle set and the values kept.
            let mut expected = at_mark.clone();
            let top = executor.top();
            let frame = expected.frames.last_mut().expect("a frame");
            (frame.1, frame.2) = (top.block.0, top.inst);
            let base = executor.top().base;
            expected.slots[base + handle.index()] = mark_value;
            let old_handle = at_mark.slots[base + handle.index()];
            expected.kinds.extend([Undo::Mark, Undo::Rewind]);
            expected.words.push(old_handle);
            expected.words.push(*after.words.last().expect("the rewind's word"));
            expected.marks.push(*after.marks.last().expect("the mark's entry"));
            for config in &mut expected.amplitudes {
                for &(column, value) in &kept {
                    config.0[column] ^= value;
                }
            }
            expected.amplitudes.sort();
            assert_eq!(after, expected, "{position:?} {assignment:?}: rewind is not the kept XOR at the mark");
            if let Some(other) = outputs.insert(after, assignment.clone()) {
                panic!("{position:?}: starts {other:?} and {assignment:?} rewind to the same state");
            }
            assert_eq!(executor.step_back(), Ok(true));
            let mut back = executor.snapshot();
            back.classical.steps = at_rewind.classical.steps;
            assert_eq!(back.classical, at_rewind.classical, "{position:?} {assignment:?}: rewind not undone");
            assert_eq!(key(&executor), at_rewind_key, "{position:?} {assignment:?}: history not restored");
            checked += 1;
        }
    }
    checked
}

/// Memory owned by one world, and memory as an overlay over a shared base whose bytes at `BYTES` are one
/// of the values a check assigns, or neither (so every assigned byte is in the overlay).
const SHARED: [Option<u8>; 3] = [None, Some(0x5a), Some(0x33)];

fn fixtures(module: &ValidModule) -> Vec<Fixture<'_>> {
    let mut fixtures = Vec::new();
    for (dirty, shared) in [false, true].into_iter().flat_map(|dirty| SHARED.map(|shared| (dirty, shared))) {
        // Down each arm of @steps (a switch default, the right arm, an exit, a fault, a division by zero)
        // and each arm of @mix.
        let runs = [
            ("steps", [1, 2]),
            ("steps", [2, 1]),
            ("steps", [0, 1]),
            ("steps", [1, 3]),
            ("steps", [3, 0]),
            ("region", [1, 2]),
            ("region", [2, 1]),
        ];
        for (name, args) in runs {
            let entry = module.function_named(name).expect("entry");
            fixtures.push(Fixture { module, entry, args: args.to_vec(), dirty, shared });
        }
    }
    fixtures
}

#[test]
fn every_journalled_step_is_a_bijection_on_clean_and_dirty_history() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let (checked, faults) = fixtures(&module)
        .iter()
        .map(check_single_steps)
        .fold((0, 0), |(c, f), (checked, faults)| (c + checked, f + faults));
    // The counts pin the coverage: a change to the program or the value sets shows here.
    assert_eq!((checked, faults), (115_080, 9_048), "single steps checked, and faulting ones among them");
}

#[test]
fn every_rewind_is_a_bijection_from_its_mark_on_clean_and_dirty_history() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let checked: usize = fixtures(&module).iter().map(check_rewinds).sum();
    assert_eq!(checked, 24_576, "rewinds checked");
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
        Undo::Mark,
        Undo::Rewind,
    ] {
        assert!(kinds.contains(&kind), "no {kind:?} step in the fixtures");
    }
    assert!(outcomes.len() >= 3, "returns, an exit and a fault: {outcomes:?}");
}

/// Worlds (`docs/DESIGN.md`, "Worlds"): a call that reads a register as an integer, and a rewind that
/// merges the worlds it split into. Each is checked on basis states (one configuration: one world) as a
/// bijection, as above, and on superpositions of them: the worlds a superposition steps to must be the
/// linear extension of the basis results, configurations of equal machine states together in one world,
/// and stepping back must return to the superposition exactly.
const WORLDS: &str = r#"
memory 256
data 16 = i8 3, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 5

func @pick(i2 %x, i2 %a) -> i2 {
entry:
    %y = add i2 %x, %a
    %c = icmp eq i2 %x, 2
    br %c, two, other
two:
    %w = zext i2 %y to i8
    store i8 %w, 24
    ret %y
other:
    ret %a
}

func @worlds(i2 %a, i2 %b) -> i2 {
entry:
    %q = qalloc 2
    %r = qalloc 2
    %m = mark
    %s = call @pick(%q, %a)
    %t = add i2 %s, %b
    rewind %m keep (%t -> %r)
    ret %a
}
"#;

/// A world's machine state without its amplitudes, and its amplitudes.
type WorldKey = (Key, Vec<(Vec<u64>, i64, i64)>);

/// Every world of the executor, each as its machine state and amplitudes, sorted.
fn world_keys(executor: &mut Executor) -> Vec<WorldKey> {
    let mut keys = Vec::new();
    let count = executor.worlds.len().max(1);
    for index in 0..count {
        if executor.has_worlds() {
            executor.select_world(index);
        }
        let mut key = key(executor);
        // A basis state stays one world, which writes its base; a superposition's worlds write overlays.
        // Both are normal (checked by `key`) over one base, so equal memory is equal representation.
        key.overlay.clear();
        // The callee's slots, dead once it returned, still hold what it read: part of the machine state.
        key.slots = executor.slots.clone();
        let end = key.slots.iter().rposition(|&slot| slot != 0).map_or(0, |last| last + 1);
        key.slots.truncate(end);
        let amplitudes = std::mem::take(&mut key.amplitudes);
        keys.push((key, amplitudes));
    }
    keys.sort();
    keys
}

/// The worlds a superposition of `basis` results gives: results with equal machine states together.
fn superposed(basis: &[(WorldKey, (f64, f64))]) -> Vec<WorldKey> {
    let mut worlds: Vec<WorldKey> = Vec::new();
    for ((machine, amplitudes), (re, im)) in basis {
        let scaled = amplitudes.iter().map(|(config, a, b)| {
            let (a, b) = (*a as f64 / 1e9, *b as f64 / 1e9);
            let rounded = |x: f64| (x * 1e9).round() as i64;
            (config.clone(), rounded(a * re - b * im), rounded(a * im + b * re))
        });
        match worlds.iter_mut().find(|(other, _)| other == machine) {
            Some((_, existing)) => existing.extend(scaled),
            None => worlds.push((machine.clone(), scaled.collect())),
        }
    }
    for (_, amplitudes) in &mut worlds {
        amplitudes.sort();
    }
    worlds.sort();
    worlds
}

/// Distinct amplitudes for the 16 configurations of two 2-bit registers, normalised.
fn weights() -> Vec<(Vec<u64>, (f64, f64))> {
    let raw: Vec<(f64, f64)> = (0..16).map(|k| (1.0 + k as f64, 0.5 * (k % 3) as f64)).collect();
    let norm = raw.iter().map(|(re, im)| re * re + im * im).sum::<f64>().sqrt();
    (0..16u64).map(|k| (vec![k & 3, k >> 2], (raw[k as usize].0 / norm, raw[k as usize].1 / norm))).collect()
}

fn set_state(executor: &mut Executor, configs: &[(Vec<u64>, (f64, f64))]) {
    let amplitudes = configs
        .iter()
        .map(|(config, (re, im))| (config.clone().into_boxed_slice(), num_complex::Complex64::new(*re, *im)))
        .collect();
    executor.quantum.replace_amplitudes(amplitudes);
}

/// One step if `through` is `start`, otherwise every world past `through`.
fn advance(executor: &mut Executor, start: usize, through: usize) {
    if start == through {
        assert_eq!(executor.step(), None, "the step runs");
    } else {
        run_past(executor, through);
    }
}

/// Steps every world until each has stepped past `instruction` (an index in the entry block), which
/// only the entry frame reaches.
fn run_past(executor: &mut Executor, instruction: usize) {
    loop {
        let mut all_past = true;
        let count = executor.worlds.len().max(1);
        for index in 0..count {
            if executor.has_worlds() {
                executor.select_world(index);
            }
            all_past &= executor.frames.len() == 1 && executor.top().inst > instruction;
        }
        if all_past {
            return;
        }
        assert_eq!(executor.step(), None, "the run goes on");
    }
}

#[test]
fn reading_a_register_and_merging_are_the_linear_extension_of_bijections() {
    let module = bennett_ir::load(WORLDS).expect("valid");
    let entry = module.function_named("worlds").expect("entry");
    let (mut reads, mut rewinds) = (0, 0);
    for (dirty, shared) in [false, true].into_iter().flat_map(|dirty| SHARED.map(|shared| (dirty, shared))) {
        let fixture = Fixture { module: &module, entry, args: vec![0, 0], dirty, shared };
        // Positions: 3 is the reading call, 2 the mark; the rewind is instruction 5.
        for (start, through) in [(3, 3), (2, 5)] {
            let mut outputs = HashMap::new();
            for a in 0..4u128 {
                for b in 0..4u128 {
                    for bytes in assignments(&[BYTE_VALUES.to_vec(), BYTE_VALUES.to_vec()]) {
                        let at = |configs: &[(Vec<u64>, (f64, f64))]| {
                            let mut executor = fixture.at(start);
                            let slots = executor.frames[0].base;
                            executor.slots[slots] = a;
                            executor.slots[slots + 1] = b;
                            executor.memory.write(BYTES[0], &[bytes[0] as u8]);
                            executor.memory.write(BYTES[1], &[bytes[1] as u8]);
                            set_state(&mut executor, configs);
                            executor
                        };
                        let mut basis = Vec::new();
                        let mut before_basis = Vec::new();
                        for (config, amplitude) in weights() {
                            let mut executor = at(&[(config.clone(), (1.0, 0.0))]);
                            let before = world_keys(&mut executor);
                            advance(&mut executor, start, through);
                            let after = world_keys(&mut executor);
                            assert_eq!(after.len(), 1, "a basis state is one world");
                            if let Some(other) = outputs.insert(after[0].clone(), (a, b, bytes.clone(), config.clone()))
                            {
                                panic!("{other:?} and {:?} step to the same state", (a, b, &bytes, &config));
                            }
                            // Back: the step at `through` and everything after `start` undone exactly.
                            let mut back = executor;
                            while back.frames.len() != 1 || back.top().inst != start {
                                assert_eq!(back.step_back(), Ok(true));
                            }
                            assert_eq!(world_keys(&mut back), before, "{config:?}: not undone exactly");
                            basis.push((after[0].clone(), amplitude));
                            before_basis.push((before[0].clone(), amplitude));
                        }
                        // All sixteen at once.
                        let mut executor = at(&weights());
                        let before = world_keys(&mut executor);
                        assert_eq!(before, superposed(&before_basis), "the superposition is one world");
                        advance(&mut executor, start, through);
                        let after = world_keys(&mut executor);
                        assert_eq!(after, superposed(&basis), "a={a} b={b} {bytes:?}: not the linear extension");
                        assert_eq!(after.len(), if through == 3 { 4 } else { 1 }, "a world per value read, merged");
                        while executor.has_worlds() || executor.frames.len() != 1 || executor.top().inst != start {
                            assert_eq!(executor.step_back(), Ok(true));
                        }
                        assert_eq!(world_keys(&mut executor), before, "the superposition is not restored");
                        if through == 3 { reads += 1 } else { rewinds += 1 }
                    }
                }
            }
        }
    }
    // a, b, two bytes, clean and dirty, memory owned or an overlay over two bases: 192 starts each, every
    // one with 16 basis states and their sum.
    assert_eq!((reads, rewinds), (384, 384));
}

/// Qubits in memory (`docs/DESIGN.md`, "Qubits in memory"): gates at addresses that promote bytes to
/// cells, at an address that depends on an argument (in bounds, out of bounds, aliasing another qubit), a
/// coherent region reading superposed cells, loads, stores and a jump record over cells, and a rewind
/// whose region promotes a byte. `assign` changes a cell's value where a varied byte holds qubits.
const CELLS: &str = r#"
memory 256
data 16 = i8 3, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 5

func @peek(i2 %x) -> i8 {
entry:
    %v = load i8 16
    %w = load i8 24
    %s = add i8 %v, %w
    %e = zext i2 %x to i8
    %t = xor i8 %s, %e
    ret %t
}

func @cells(i2 %a, i2 %b) -> i2 {
entry:
    %q = qalloc 2
    %o = qalloc 8
    %e = qalloc 5
    %p = zext i2 %a to i64
    %addr = add i64 %p, 16
    %far = mul i64 %p, 120
    h [16:1]
    x [%addr:0] ctrl [16:1]
    x [%addr:1] ctrl [16:1]
    x %q[0] ctrl [16:1]
    s [24:3] ctrl %q[0]
    t [%far:2]
    apply @peek(%b) -> %o, %e budget 20
    apply @peek(%b) -> %o, %e budget 20
    tdg [%far:2]
    sdg [24:3] ctrl %q[0]
    x %q[0] ctrl [16:1]
    x [%addr:1] ctrl [16:1]
    x [%addr:0] ctrl [16:1]
    h [16:1]
    %l = load i16 16
    %n = trunc i16 %l to i2
    store i16 %l, 23
    %j = setjmp 16
    %h = mark
    x [24:5]
    h [40:0]
    x [40:0] ctrl [24:5]
    rewind %h keep (%n -> %q)
    ret %n
}
"#;

fn cell_fixtures(module: &ValidModule) -> Vec<Fixture<'_>> {
    let entry = module.function_named("cells").expect("entry");
    let mut fixtures = Vec::new();
    for (dirty, shared) in [false, true].into_iter().flat_map(|dirty| SHARED.map(|shared| (dirty, shared))) {
        // In bounds at two addresses; `[%addr:1]` aliasing `[16:1]` (a = 0); out of bounds (a = 3).
        for args in [[1, 2], [2, 1], [0, 1], [3, 0]] {
            fixtures.push(Fixture { module, entry, args: args.to_vec(), dirty, shared });
        }
    }
    fixtures
}

#[test]
fn every_step_at_memory_qubits_is_a_bijection_on_clean_and_dirty_history() {
    let module = bennett_ir::load(CELLS).expect("valid");
    let fixtures = cell_fixtures(&module);
    let (checked, faults) =
        fixtures.iter().map(check_single_steps).fold((0, 0), |(c, f), (checked, faults)| (c + checked, f + faults));
    let rewinds: usize = fixtures.iter().map(check_rewinds).sum();
    let mut kinds = std::collections::BTreeSet::new();
    let mut outcomes = std::collections::BTreeSet::new();
    for fixture in &fixtures {
        let mut executor = fixture.at(0);
        outcomes.insert(format!("{:?}", executor.run()));
        let journal = executor.journal.as_ref().expect("reversible");
        kinds.extend((0..journal.records()).map(|index| journal.kind_at(index)));
    }
    assert!(
        kinds.contains(&Undo::GateAt)
            && kinds.contains(&Undo::GateAtPromoted)
            && kinds.contains(&Undo::Setjmp)
            && kinds.contains(&Undo::Rewind)
    );
    assert!(outcomes.iter().any(|o| o.contains("QubitAlias")) && outcomes.iter().any(|o| o.contains("OutOfBounds")));
    println!("memory qubits: {checked} single steps ({faults} faulting), {rewinds} rewinds; outcomes {outcomes:?}");
    assert_eq!((checked, faults, rewinds), (8_616, 552, 12_288), "single steps, faulting ones, rewinds");
}

/// Reading a cell (a load, or a store over it) where the configurations disagree splits the world, and
/// the rewind merges the worlds again: as for a register read, the linear extension of bijections. The
/// register `%r` is column 0 and the cell at 40 column 1, so `weights` sets both.
fn cell_worlds(first_access: &str) -> String {
    format!(
        r#"
memory 256
data 16 = i8 3, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 0, i8 5

func @pick(i2 %x, i2 %a) -> i2 {{
entry:
    %y = add i2 %x, %a
    %c = icmp eq i2 %x, 2
    br %c, two, other
two:
    %w = zext i2 %y to i8
    store i8 %w, 24
    ret %y
other:
    ret %a
}}

func @cellworlds(i2 %a, i2 %b) -> i2 {{
entry:
    %r = qalloc 2
    z [40]
    %m = mark
{first_access}
    %t = trunc i8 %l to i2
    %s = call @pick(%t, %a)
    %u = add i2 %s, %b
    %w = zext i2 %u to i8
    store i8 %w, 40
    rewind %m keep (%u -> %r)
    ret %a
}}
"#
    )
}

#[test]
fn reading_a_memory_qubit_and_merging_are_the_linear_extension_of_bijections() {
    let accesses = [
        "    %l = load i8 40",
        // A store over the cell reads it first (its old value goes into the history), then the load.
        "    %k = zext i2 %b to i8\n    store i8 %k, 40\n    %l = load i8 40",
    ];
    let (mut reads, mut rewinds) = (0, 0);
    for access in accesses {
        let text = cell_worlds(access);
        let module = bennett_ir::load(&text).expect("valid");
        let entry = module.function_named("cellworlds").expect("entry");
        let first = if access.contains("store") { 4 } else { 3 };
        let rewind = module.function(entry).blocks[0].insts.len() - 1;
        for (dirty, shared) in [false, true].into_iter().flat_map(|dirty| SHARED.map(|shared| (dirty, shared))) {
            let fixture = Fixture { module: &module, entry, args: vec![0, 0], dirty, shared };
            for (start, through) in [(first, first), (2, rewind)] {
                let mut outputs = HashMap::new();
                for a in 0..4u128 {
                    for b in 0..4u128 {
                        let at = |configs: &[(Vec<u64>, (f64, f64))]| {
                            let mut executor = fixture.at(start);
                            let slots = executor.frames[0].base;
                            executor.slots[slots] = a;
                            executor.slots[slots + 1] = b;
                            set_state(&mut executor, configs);
                            executor
                        };
                        let mut basis = Vec::new();
                        let mut before_basis = Vec::new();
                        for (config, amplitude) in weights() {
                            let mut executor = at(&[(config.clone(), (1.0, 0.0))]);
                            let before = world_keys(&mut executor);
                            advance(&mut executor, start, through);
                            let after = world_keys(&mut executor);
                            assert_eq!(after.len(), 1, "a basis state is one world");
                            if let Some(other) = outputs.insert(after[0].clone(), (a, b, config.clone())) {
                                panic!("{other:?} and {:?} step to the same state", (a, b, &config));
                            }
                            let mut back = executor;
                            while back.frames.len() != 1 || back.top().inst != start {
                                assert_eq!(back.step_back(), Ok(true));
                            }
                            assert_eq!(world_keys(&mut back), before, "{config:?}: not undone exactly");
                            basis.push((after[0].clone(), amplitude));
                            before_basis.push((before[0].clone(), amplitude));
                        }
                        let mut executor = at(&weights());
                        let before = world_keys(&mut executor);
                        assert_eq!(before, superposed(&before_basis), "the superposition is one world");
                        advance(&mut executor, start, through);
                        let after = world_keys(&mut executor);
                        assert_eq!(after, superposed(&basis), "a={a} b={b}: not the linear extension");
                        assert_eq!(after.len(), if through == first { 4 } else { 1 }, "a world per value read, merged");
                        while executor.has_worlds() || executor.frames.len() != 1 || executor.top().inst != start {
                            assert_eq!(executor.step_back(), Ok(true));
                        }
                        assert_eq!(world_keys(&mut executor), before, "the superposition is not restored");
                        if through == first { reads += 1 } else { rewinds += 1 }
                    }
                }
            }
        }
    }
    // Two first accesses (a load, a store), a and b, clean and dirty, three memory modes: 96 starts each,
    // every one with 16 basis states and their sum.
    assert_eq!((reads, rewinds), (192, 192));
}
