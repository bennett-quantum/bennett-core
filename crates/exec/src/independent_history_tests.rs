//! Independent whole-state adapter for the earlier finite checker. Lives below `quantum` only to
//! inspect allocation history as well as amplitudes; no production state/step API is added.
//! DESIGN.md “History” promises a bijection on history-consistent rewinds, not arbitrary mismatches.

#[path = "../tests/oracle_support/permutation.rs"]
mod permutation;

use std::collections::BTreeMap;

use bennett_ir::{Fault, Inst};

use crate::executor::{Executor, Options, Outcome};
use crate::journal::{MarkEntry, Undo};
use permutation::{check_bijection, words};

// Work counters and peaks are observations, not coordinates of the reversible machine.
// This fixture never measures: RNG and run options remain fixed across the entire domain.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct State {
    memory: Vec<u8>,
    frames: Vec<(u32, u32, usize, usize, u64)>,
    slots: Vec<u128>,
    stack: u64,
    tape: Vec<u8>,
    outcome: String,
    kinds: Vec<Undo>,
    history: Vec<u128>,
    marks: Vec<(usize, u64)>,
    registers: Vec<(u32, Option<u32>)>,
    columns: usize,
    allocations: u64,
    allocation_numbers: Vec<Vec<u64>>,
    amplitudes: Vec<(Vec<u64>, i64, i64)>,
}

fn state(e: &Executor<'_>) -> State {
    let j = e.journal.as_ref().expect("journal");
    let q = &e.quantum;
    State {
        memory: e.memory.clone(),
        frames: e.frames.iter().map(|f| (f.func.0, f.block.0, f.inst, f.base, f.entry_stack_pointer)).collect(),
        slots: e.slots[..e.live_top()].to_vec(),
        stack: e.stack_pointer,
        tape: e.tape.clone(),
        outcome: format!("{:?}", e.outcome()),
        kinds: (0..j.records()).map(|i| j.kind_at(i)).collect(),
        history: j.words().to_vec(),
        marks: j.marks().iter().map(|m| (m.position, m.allocations)).collect(),
        registers: q.registers.iter().map(|r| (r.generation, r.width.map(|w| w.bits()))).collect(),
        columns: q.columns,
        allocations: q.allocations,
        allocation_numbers: q.allocation_numbers.clone(),
        amplitudes: q
            .amplitudes
            .iter()
            .map(|(c, a)| (c.to_vec(), (a.re * 1e12).round() as i64, (a.im * 1e12).round() as i64))
            .collect(),
    }
}

const PROGRAM: &str = r#"
memory 256
func @main(i2 %a) {
entry:
    %q = qalloc 2
    %h = mark
    store i2 %a, 16
    %g = mark
    %v = add i2 %a, 1
    rewind %g keep (%v -> %q, i2 3 -> %q)
    rewind %h keep (%a -> %q)
    ret
}
"#;

/// Full Cartesian variation of data, overwritten slots, memory, tape and garbage-prefix words.
/// Dirty prefixes include three nested marks and every encoding of three independent one-bit words.
#[test]
fn independent_checker_exhausts_mark_and_nested_rewind_on_clean_and_dirty_history() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let f = &module.functions[0];
    let slot = |at: usize| f.blocks[0].insts[at].result().expect("result");
    let mut total = 0;
    for dirty in [false, true] {
        let domain = if dirty { words(&[2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1]) } else { words(&[2, 2, 1, 1, 1, 1, 1, 1]) };
        let mut starts = Vec::new();
        let mut expected_marks = Vec::new();
        let mut expected_rewinds = Vec::new();
        let mut mark_images = BTreeMap::new();
        let mut rewind_images = BTreeMap::new();
        for input in domain {
            let mut e = Executor::new(
                &module,
                module.function_named("main").expect("main"),
                &[input[0]],
                Options { reversible: true, budget: Some(100), ..Options::default() },
            )
            .expect("start");
            assert_eq!(e.step(), None); // qalloc
            let handle = e.slots[slot(0).index()] as u64;
            e.quantum.xor_register(e.quantum.resolve(handle).expect("q"), input[1]);
            for (at, &value) in [1, 3, 4].into_iter().zip(&input[2..5]) {
                e.slots[slot(at).index()] = value.into();
            }
            e.memory[16] = input[5] as u8;
            e.memory[17] = input[6] as u8;
            e.tape.push(input[7] as u8);
            if dirty {
                let j = e.journal.as_mut().expect("journal");
                // Opaque garbage is below the tested region and must survive byte for byte.
                for &word in &input[8..11] {
                    j.push_word(word.into());
                    j.push(Undo::Mark);
                    j.push_mark(MarkEntry { position: j.records(), allocations: 1 });
                }
            }
            let before = state(&e);
            let mut mark = before.clone();
            let position = mark.kinds.len() + 1;
            mark.history.push(mark.slots[slot(1).index()]);
            mark.kinds.push(Undo::Mark);
            mark.marks.push((position, 1));
            mark.slots[slot(1).index()] = position as u128;
            mark.frames[0].2 += 1;
            assert_eq!(e.step(), None);
            let at_mark = state(&e);
            assert_eq!(at_mark, mark, "mark: input {input:?}");
            assert_eq!(e.step_back(), Ok(true));
            assert_eq!(state(&e), before);
            assert_eq!(e.step(), None);
            let mut inner_count = 0;
            loop {
                let frame = e.frames[0];
                if frame.inst == 6 {
                    break;
                }
                let before_step = state(&e);
                let steps = e.stats().steps;
                assert_eq!(e.step(), None);
                let after_step = state(&e);
                // Also independently compare the full single-step inverse at the nested rewind.
                if matches!(f.blocks[0].insts[frame.inst], Inst::Rewind { .. }) {
                    assert_eq!(e.step_back(), Ok(true));
                    assert_eq!(state(&e), before_step);
                    assert_eq!(e.stats().steps, steps);
                    assert_eq!(e.step(), None);
                    assert_eq!(state(&e), after_step);
                    inner_count += 1;
                }
            }
            assert_eq!(inner_count, 1);
            let before_outer = state(&e);
            assert_eq!(e.step(), None);
            let after_outer = state(&e);
            let mut expected = mark.clone();
            expected.frames[0].2 = 7;
            expected.amplitudes[0].0[0] ^= input[0];
            // Rewind leaves only the mark plus a record of where that mark left its frame, and of the
            // four steps its region takes to run again (store, inner mark, add, inner rewind).
            expected.kinds.push(Undo::Rewind);
            expected.history.push(4 << 64 | 2);
            assert_eq!(after_outer, expected, "outer: input {input:?}");
            assert_eq!(e.step_back(), Ok(true));
            assert_eq!(state(&e), before_outer, "undo outer: input {input:?}");
            starts.push(before.clone());
            expected_marks.push(mark);
            expected_rewinds.push(expected);
            mark_images.insert(before.clone(), at_mark);
            rewind_images.insert(before, after_outer);
        }
        total += check_bijection(&starts, &expected_marks, |s| mark_images[s].clone()).expect("mark bijection");
        total +=
            check_bijection(&starts, &expected_rewinds, |s| rewind_images[s].clone()).expect("nested rewind bijection");
    }
    assert_eq!(total, 18432);
    println!("{total} independent full-state mark/nested-rewind images, including allocation stacks and dirty words");
}

/// Fault boundaries are part of the step's domain, rather than silently excluded.
#[test]
fn independent_checker_includes_stale_handle_fault_states_without_erasing_history() {
    let module = bennett_ir::load(PROGRAM).expect("valid");
    let mut source = Vec::new();
    let mut target = Vec::new();
    let mut images = BTreeMap::new();
    for values in words(&[2, 2, 2]) {
        let mut e = Executor::new(
            &module,
            module.function_named("main").expect("main"),
            &[values[0]],
            Options { reversible: true, ..Options::default() },
        )
        .expect("start");
        assert_eq!(e.step(), None);
        let Inst::Mark { dst } = module.functions[0].blocks[0].insts[1] else { panic!("mark") };
        e.slots[dst.index()] = 0; // stale: mark not run
        e.frames[0].inst = 6;
        e.memory[16] = values[1] as u8;
        let journal = e.journal.as_mut().expect("journal");
        journal.push_word(values[2].into());
        journal.push(Undo::Store);
        let before = state(&e);
        let mut expected = before.clone();
        expected.outcome = format!("{:?}", Some(Outcome::Faulted(Fault::BadRewind)));
        assert_eq!(e.step(), Some(Outcome::Faulted(Fault::BadRewind)));
        let after = state(&e);
        assert_eq!(after, expected);
        source.push(before.clone());
        target.push(expected);
        images.insert(before, after);
    }
    assert_eq!(check_bijection(&source, &target, |s| images[s].clone()), Ok(64));
}
