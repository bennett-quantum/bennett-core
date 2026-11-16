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
    cells: Vec<(u64, usize)>,
    columns: usize,
    allocations: u64,
    allocation_numbers: Vec<Vec<u64>>,
    amplitudes: Vec<(Vec<u64>, i64, i64)>,
}

fn state(e: &Executor<'_>) -> State {
    let j = e.journal.as_ref().expect("journal");
    let q = &e.quantum;
    State {
        memory: e.memory.to_vec(),
        frames: e.frames.iter().map(|f| (f.func.0, f.block.0, f.inst, f.base, f.entry_stack_pointer)).collect(),
        slots: e.slots[..e.live_top()].to_vec(),
        stack: e.stack_pointer,
        tape: e.tape.clone(),
        outcome: format!("{:?}", e.outcome()),
        kinds: (0..j.records()).map(|i| j.kind_at(i)).collect(),
        history: j.words().to_vec(),
        marks: j.marks().iter().map(|m| (m.position, m.allocations)).collect(),
        registers: q.registers.iter().map(|r| (r.generation, r.width.map(|w| w.bits()))).collect(),
        cells: q.cells.iter().map(|(&a, &id)| (a, id)).collect(),
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
            e.memory.write(16, &[input[5] as u8]);
            e.memory.write(17, &[input[6] as u8]);
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
            // Rewind leaves only the mark plus a record of where that mark left its frame, and that this is
            // the first visit of the rewind since the mark (running its region again stops there).
            expected.kinds.push(Undo::Rewind);
            expected.history.push(1 << 64 | 2);
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
        e.memory.write(16, &[values[1] as u8]);
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

/// All private machine coordinates, including dead slots and read/split and jump metadata.
/// Partition flags describe how a superposition is represented; the analytic linear-extension
/// comparison sets them on its expected reading-call records when that input actually splits.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WholeWorld {
    state: State,
    reads: Vec<(usize, usize, u32, bool)>,
    jumps: Vec<usize>,
    cells: Vec<(usize, usize, u8, bool)>,
}

fn whole_worlds(e: &mut Executor<'_>) -> Vec<WholeWorld> {
    let mut worlds = Vec::new();
    for i in 0..e.worlds.len().max(1) {
        if e.has_worlds() {
            e.select_world(i);
        }
        let mut s = state(e);
        s.slots = e.slots.clone();
        while s.slots.last() == Some(&0) {
            s.slots.pop();
        }
        let j = e.journal.as_ref().expect("journal");
        worlds.push(WholeWorld {
            state: s,
            reads: j.reads_from(0).iter().map(|r| (r.record, r.column, r.slot.0, r.split)).collect(),
            jumps: j.jumps().to_vec(),
            cells: j.cell_entries().iter().map(|c| (c.record, c.column, c.value, c.split)).collect(),
        });
    }
    worlds.sort();
    worlds
}

#[test]
fn independent_whole_state_split_merge_checker_includes_dirty_history_and_all_private_metadata() {
    let p = bennett_ir::load(
        r#"
memory 512
func @f(i2 %q, i1 %r, i2 %a)->i2 {
entry:
%c = icmp eq i2 %q, 2
br %c, two, other
two:
%w = zext i1 %r to i2
%x = xor i2 %q, %w
store i2 %x, 16
%v = add i2 %x, %a
ret %v
other:
ret %q
}
func @main(i2 %a) {
entry:
%q = qalloc 2
%r = qalloc 1
%o = qalloc 2
%m = mark
%v = call @f(%q, %r, %a)
rewind %m keep (%v -> %o)
ret
}
"#,
    )
    .expect("private checker fixture");
    let (mut starts, mut images, mut map) = (Vec::new(), Vec::new(), BTreeMap::new());
    let mut total = 0;
    for merging in [false, true] {
        for dirty in [false, true] {
            for a in 0..4 {
                for byte in 0..4 {
                    let prepare = |configs: Vec<(Vec<u64>, num_complex::Complex64)>| {
                        let mut e = Executor::new(
                            &p,
                            p.function_named("main").expect("main"),
                            &[a],
                            Options { reversible: true, budget: Some(1000), ..Options::default() },
                        )
                        .expect("start");
                        for _ in 0..3 {
                            assert_eq!(e.step(), None);
                        }
                        e.memory.write(16, &[byte]);
                        if dirty {
                            let j = e.journal.as_mut().expect("journal");
                            j.push_word(0xdead);
                            j.push(Undo::Store);
                            j.push_word(0xbeef);
                            j.push(Undo::Jump);
                        }
                        e.quantum
                            .replace_amplitudes(configs.into_iter().map(|(c, a)| (c.into_boxed_slice(), a)).collect());
                        if !merging {
                            assert_eq!(e.step(), None);
                        }
                        e
                    };
                    let mut basis_results = Vec::new();
                    let advance = |e: &mut Executor<'_>| {
                        if !merging {
                            assert_eq!(e.step(), None);
                            return;
                        }
                        for _ in 0..100 {
                            let done =
                                whole_worlds(e).iter().all(|w| w.state.frames.len() == 1 && w.state.frames[0].2 == 6);
                            if done {
                                return;
                            }
                            assert_eq!(e.step(), None);
                        }
                        panic!("merge did not finish");
                    };
                    for k in 0..32u64 {
                        let config = vec![k & 3, (k >> 2) & 1, k >> 3];
                        let mut e = prepare(vec![(config, num_complex::Complex64::new(1.0, 0.0))]);
                        let before = whole_worlds(&mut e);
                        advance(&mut e);
                        let after = whole_worlds(&mut e);
                        assert_eq!(after.len(), 1);
                        // Independent classical result, including the unequal branch and per-world store.
                        if merging {
                            let f = if k & 3 == 2 { (((k & 3) ^ ((k >> 2) & 1)) + a) & 3 } else { k & 3 };
                            assert_eq!(after[0].state.amplitudes[0].0, vec![k & 3, (k >> 2) & 1, (k >> 3) ^ f]);
                            assert_eq!(after[0].state.memory[16], byte);
                        }
                        starts.push(before.clone());
                        images.push(after.clone());
                        map.insert(before.clone(), after.clone());
                        basis_results.push(after[0].clone());
                        let target = if merging { 3 } else { 4 };
                        while e.has_worlds() || e.frames.len() != 1 || e.top().inst != target {
                            assert_eq!(e.step_back(), Ok(true));
                        }
                        assert_eq!(whole_worlds(&mut e), before, "basis inverse, k={k}, dirty={dirty}");
                    }
                    let scale = (1.0 / 32.0f64).sqrt();
                    let configs = (0..32u64)
                        .map(|k| {
                            (
                                vec![k & 3, (k >> 2) & 1, k >> 3],
                                num_complex::Complex64::new(if k & 1 == 0 { scale } else { -scale }, 0.0),
                            )
                        })
                        .collect();
                    let mut e = prepare(configs);
                    let before = whole_worlds(&mut e);
                    advance(&mut e);
                    let after = whole_worlds(&mut e);
                    let mut expected = BTreeMap::<WholeWorld, Vec<(Vec<u64>, i64, i64)>>::new();
                    for (k, mut w) in basis_results.into_iter().enumerate() {
                        let mut amp = std::mem::take(&mut w.state.amplitudes);
                        if !merging {
                            for read in &mut w.reads {
                                read.3 = true;
                            }
                        }
                        for (_, re, im) in &mut amp {
                            *re = (*re as f64 * if k & 1 == 0 { scale } else { -scale }).round() as i64;
                            *im = 0;
                        }
                        expected.entry(w).or_default().extend(amp);
                    }
                    let mut expected: Vec<_> = expected
                        .into_iter()
                        .map(|(mut w, mut a)| {
                            a.sort();
                            w.state.amplitudes = a;
                            w
                        })
                        .collect();
                    expected.sort();
                    assert_eq!(after, expected, "linear extension dirty={dirty}, a={a}, byte={byte}");
                    while e.has_worlds() || e.frames.len() != 1 || e.top().inst != if merging { 3 } else { 4 } {
                        assert_eq!(e.step_back(), Ok(true));
                    }
                    assert_eq!(whole_worlds(&mut e), before, "superposition inverse");
                }
            }
        }
        total +=
            check_bijection(&starts, &images, |s| map[s].clone()).expect("whole-state bijection on enumerated images");
        starts.clear();
        images.clear();
        map.clear();
    }
    assert_eq!(total, 2048);
    println!(
        "2048 full private split/merge images; 64 signed 32-basis superpositions; clean/dirty histories; exact inverses"
    );
}

#[test]
fn independent_memory_composite_bijection_includes_cell_table_dead_slots_and_full_journal() {
    use num_complex::Complex64 as C;
    let p = bennett_ir::load("memory 512\nfunc @main() {\nentry:\n%q = qalloc 2\nz [40]\n%m = mark\n%l = load i8 40\n%r = trunc i8 %l to i2\nstore i8 3, 40\nh %q\nrewind %m keep (%r -> %q)\nret\n}\n").expect("valid");
    let (mut starts, mut images, mut map) = (Vec::new(), Vec::new(), BTreeMap::new());
    let mut linear = 0;
    for dirty in [false, true] {
        for byte in 0..4 {
            let prepare = |configs: Vec<(Vec<u64>, C)>| {
                let mut e = Executor::new(
                    &p,
                    p.function_named("main").expect("main"),
                    &[],
                    Options { reversible: true, ..Options::default() },
                )
                .expect("starts");
                assert_eq!(e.step(), None);
                assert_eq!(e.step(), None);
                e.memory.write(41, &[byte]);
                // A dead slot and every private journal coordinate survive the composite.
                e.slots.push(0x1234 + u128::from(byte));
                if dirty {
                    let j = e.journal.as_mut().expect("journal");
                    j.push_word(0xdead);
                    j.push(Undo::Store);
                    j.push_word(0xbeef);
                    j.push(Undo::Jump);
                }
                e.quantum.replace_amplitudes(configs.into_iter().map(|(c, a)| (c.into_boxed_slice(), a)).collect());
                e
            };
            let advance = |e: &mut Executor<'_>| {
                for _ in 0..30 {
                    if whole_worlds(e).iter().all(|w| w.state.frames[0].2 == 8) {
                        return;
                    }
                    assert_eq!(e.step(), None);
                }
                panic!("rewind did not finish");
            };
            let mut basis_images = Vec::new();
            for q in 0..4 {
                for cell in 0..4 {
                    let mut e = prepare(vec![(vec![q, cell], C::new(1.0, 0.0))]);
                    let before = whole_worlds(&mut e);
                    advance(&mut e);
                    let after = whole_worlds(&mut e);
                    assert_eq!(after.len(), 1);
                    let phase = if q & cell & 1 == 1 { -1_000_000_000_000 } else { 1_000_000_000_000 };
                    assert_eq!(after[0].state.amplitudes, vec![(vec![q ^ (cell & 2), cell], phase, 0)]);
                    assert_eq!(after[0].state.cells, vec![(40, 1)]);
                    assert_eq!(after[0].state.memory[40], 0);
                    assert_eq!(after[0].state.memory[41], byte);
                    starts.push(before.clone());
                    images.push(after.clone());
                    map.insert(before.clone(), after.clone());
                    basis_images.push(after[0].clone());
                    while e.has_worlds() || e.top().inst != 2 {
                        assert_eq!(e.step_back(), Ok(true));
                    }
                    assert_eq!(whole_worlds(&mut e), before);
                }
            }
            let configs =
                (0..16).map(|k| (vec![k / 4, k % 4], C::new(if k & 2 == 0 { 0.25 } else { -0.25 }, 0.0))).collect();
            let mut e = prepare(configs);
            let before = whole_worlds(&mut e);
            advance(&mut e);
            let after = whole_worlds(&mut e);
            let mut amplitudes = BTreeMap::new();
            for (k, w) in basis_images.iter().enumerate() {
                let (c, re, _) = &w.state.amplitudes[0];
                amplitudes.insert(c.clone(), *re / 4 * if k & 2 == 0 { 1 } else { -1 });
            }
            assert_eq!(after.len(), 1);
            assert_eq!(after[0].state.amplitudes, amplitudes.into_iter().map(|(c, re)| (c, re, 0)).collect::<Vec<_>>());
            while e.has_worlds() || e.top().inst != 2 {
                assert_eq!(e.step_back(), Ok(true));
            }
            assert_eq!(whole_worlds(&mut e), before);
            linear += 1;
        }
    }
    assert_eq!(check_bijection(&starts, &images, |s| map[s].clone()), Ok(128));
    println!("128 complete memory composite images and {linear} signed 16-basis linear extensions invert exactly");
}
