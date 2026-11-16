//! Independent byte-image, whole-world and cost checks for DESIGN.md "Memory as an overlay".
#[allow(dead_code, reason = "shared independent oracle helpers")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome, ReverseError, Snapshot};
use bennett_ir::{Inst, ValidModule};
use oracle_support::worlds::{Model, basis};
use oracle_support::worlds_executor::{assert_observed, assert_worlds, observed_reference};

fn options() -> Options {
    Options { reversible: true, budget: Some(10000), ..Options::default() }
}

fn worlds(s: &Snapshot) -> impl Iterator<Item = &Snapshot> {
    std::iter::once(s).chain(&s.worlds)
}

/// Independent accounting from full byte images: retain the last sole world's memory as the base;
/// count unequal bytes while shared, and copy counts from the pre-call world images. The generated
/// reading calls all split a previously unread uniform qubit into two parts. No production overlay,
/// hash or memory helper participates in the expected counts.
fn audit(p: &ValidModule, e: &mut Executor<'_>) -> usize {
    let mut base = e.snapshot().classical.memory;
    let model = Model::new(p, "main");
    let mut reference = basis(model.initial("main", &[]));
    let mut compare_prefix = true;
    let (mut peak, mut copied, mut ticks) = (0, 0, 0);
    loop {
        let before = e.snapshot();
        let old_splits = e.stats().splits;
        if compare_prefix {
            assert_worlds(&model, &reference, &before);
            // The completed joined reference handles rewind parking/observation below; its simple
            // step interpretation suffices for every complete memory/frame/slot prefix before that.
            compare_prefix = !worlds(&before).any(|w| {
                w.classical.frames.last().is_some_and(|f| {
                    matches!(
                        p.function(f.func).block(f.block).insts.get(f.inst),
                        Some(Inst::Rewind { .. } | Inst::Measure { .. })
                    )
                })
            });
            if compare_prefix {
                reference = model.step(&reference);
            }
        }
        let pending: u64 = worlds(&before)
            .map(|w| {
                let Some(f) = w.classical.frames.last() else { return 0 };
                if !matches!(p.function(f.func).block(f.block).insts.get(f.inst), Some(Inst::Call { .. })) {
                    return 0;
                }
                w.classical.memory.iter().zip(&base).filter(|(a, b)| a != b).count() as u64
            })
            .sum();
        let done = e.step();
        ticks += 1;
        let s = e.snapshot();
        let stats = e.stats();
        if stats.splits > old_splits {
            if pending > 0 {
                copied += pending;
            } else if worlds(&before).all(|w| {
                w.classical.frames.last().is_some_and(|f| {
                    matches!(p.function(f.func).block(f.block).insts.get(f.inst), Some(Inst::Rewind { .. }))
                })
            }) {
                // These generated rewinds replay the same one-qubit read in each original world;
                // no write lies between that inner mark and its read. Count payload from the full
                // pre-rewind images; each original world replays the same one-qubit split.
                let sum: usize =
                    worlds(&before).map(|w| w.classical.memory.iter().zip(&base).filter(|(a, b)| a != b).count()).sum();
                assert_eq!(
                    stats.splits - old_splits,
                    (before.worlds.len() + 1) as u64,
                    "one replay split per original world"
                );
                copied += sum as u64;
            }
        }
        if s.worlds.is_empty() {
            base.clone_from(&s.classical.memory);
        }
        let held: usize =
            worlds(&s).map(|w| w.classical.memory.iter().zip(&base).filter(|(a, b)| a != b).count()).sum();
        peak = peak.max(held);
        assert_eq!(stats.overlay_bytes, held, "tick {ticks}");
        assert_eq!(stats.split_memory_bytes, copied, "tick {ticks}");
        assert_eq!(stats.peak_overlay_bytes, peak, "tick {ticks}");
        assert!(ticks < 1000);
        if done.is_some() {
            break;
        }
    }
    ticks
}

/// All device widths, overlapping accesses, restoration of an original byte, zero data, and both
/// memory ends. A second read splits after the writes, so it copies a nonempty overlay. Rewinding the
/// inner mark twice also exercises replay of a rewind while those writes remain in the overlay.
fn generated(width: u32, offset: usize) -> String {
    let n = width.div_ceil(8) as usize;
    let convert = if width == 1 { "%b = add i1 %x, 0".to_owned() } else { format!("%b = zext i1 %x to i{width}") };
    let mask = u128::MAX >> (128 - width);
    let value = 0xb3a6_395c_91e2_075d_b3a6_395c_91e2_075d_u128 & mask;
    format!(
        "memory 1024
data 0 = i64 1234605616436508552, zero 80
func @id(i1 %x) -> i1 {{
entry:
ret %x
}}
func @work(i1 %x) -> i1 {{
entry:
{convert}
%v = xor i{width} %b, {value}
store i{width} %v, {offset}
%a = load i{width} {offset}
store i{width} %v, {end}
%z = load i{width} {end}
store i8 255, {overlap}
store i8 0, {overlap}
%stack = alloca 48
store i{width} %v, %stack
%loaded = load i{width} %stack
%good = icmp eq i{width} %loaded, %v
%same = icmp eq i{width} %a, %z
%out = and i1 %same, %good
ret %out
}}
func @main() {{
entry:
%q = qalloc 1
%r = qalloc 1
%out = qalloc 1
h %q
h %r
%m = mark
%v = call @work(%q)
%n = mark
%w = call @id(%r)
rewind %n
rewind %n
rewind %m keep (%v -> %out)
h %q
h %r
ret
}}",
        end = 1024 - n,
        overlap = offset + n / 2
    )
}

fn check(width: u32, offset: usize) -> usize {
    let text = generated(width, offset);
    let p = bennett_ir::load(&text).expect("generated program valid");
    let model = Model::new(&p, "main");
    let reference = model.run_joined(basis(model.initial("main", &[])), 1000);
    let mut e = Executor::new(&p, p.function_named("main").expect("main"), &[], options()).expect("start");
    let start = e.snapshot();
    let ticks = audit(&p, &mut e);
    assert_eq!(e.outcome(), Some(Outcome::Returned(None)), "width {width}, offset {offset}");
    assert_worlds(&model, &reference, &e.snapshot());
    assert_eq!(e.snapshot().classical.memory, start.classical.memory);
    assert!((e.snapshot().quantum.amplitude(&[0, 0, 1]).re - 1.0).abs() < 1e-11);
    e.reverse().expect("reverse");
    assert_eq!(e.snapshot().classical, start.classical);
    assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-11));
    assert_eq!(e.stats().overlay_bytes, 0);
    ticks
}

#[test]
fn widths_overlaps_ends_stack_and_nested_rewinds_match_the_independent_worlds() {
    let ticks: usize = [1, 7, 8, 9, 16, 31, 32, 63, 64, 65, 127, 128].into_iter().map(|w| check(w, 1)).sum();
    println!("12 width/overlap/stack/rewind reference runs; {ticks} audited ticks");
}

/// cargo test -p bennett-exec --test memory_independent every_width -- --ignored --nocapture
#[test]
#[ignore = "all 128 widths at four unaligned offsets, with whole-world replay and exact byte counts"]
fn every_width_and_unaligned_offset() {
    let mut ticks = 0;
    for width in 1..=128 {
        for offset in [0, 1, 7, 63] {
            ticks += check(width, offset);
        }
    }
    println!("512 width/offset programs; {ticks} audited ticks and complete reversals");
}

const OBSERVE: &str = "memory 512
data 0 = i64 1234605616436508552, zero 64
func @id(i1 %x)->i1 {
entry:
ret %x
}
func @write(i1 %x)->i1 {
entry:
%v = zext i1 %x to i64
%b = add i64 %v, 257
store i64 %b, 1
ret %x
}
func @main()->i1 {
entry:
%q = qalloc 1
%r = qalloc 1
h %q
%v = call @write(%q)
%m = measure %q
h %r
%n = mark
%w = call @id(%r)
store i8 99, 7
rewind %n
h %r
ret %m
}
";

#[test]
fn sampling_preserves_memory_and_measurement_folds_before_a_new_split() {
    for measure in [true, false] {
        let text = if measure { OBSERVE.to_owned() } else { OBSERVE.replace("%m = measure %q", "%m = add i1 %v, 0") };
        let p = bennett_ir::load(&text).expect("valid");
        let model = Model::new(&p, "main");
        let reference = observed_reference(&model, basis(model.initial("main", &[])), 1000);
        let mut bins = [0; 2];
        for seed in 0..128 {
            let mut e = Executor::new(&p, p.function_named("main").expect("main"), &[], Options { seed, ..options() })
                .expect("start");
            audit(&p, &mut e);
            let Some(Outcome::Returned(Some(v))) = e.outcome() else { panic!("return") };
            bins[v as usize] += 1;
            let mut expected = vec![0; 512];
            expected[..8].copy_from_slice(&1234605616436508552_u64.to_le_bytes());
            expected[1..9].copy_from_slice(&(257 + v as u64).to_le_bytes());
            assert_eq!(e.snapshot().classical.memory, expected);
            assert_observed(&model, &reference, &e);
            assert_eq!(e.stats().overlay_bytes, 0);
            assert_eq!(e.reverse(), Err(ReverseError::PastMeasurement));
        }
        assert!(bins.iter().all(|&n| n > 32), "both memories sampled: {bins:?}");
        println!("128 sampling/fold/new-split runs (measure={measure}): {bins:?}");
    }
}

const JUMPS: &str = "memory 512
data 0 = zero 80
func @throw(i64 %buf, i32 %v) {
entry:
%p = alloca 19
store i128 226943873990930480490908391077492263160, %p
longjmp %buf, %v
}
func @work(i1 %v)->i1 {
entry:
%pad = alloca 64
%buf = add i64 %pad, 1
%r = setjmp %buf
%first = icmp eq i32 %r, 0
br %first, jump, done
jump:
%n = zext i1 %v to i32
%value = add i32 %n, 1
call @throw(%buf, %value)
ret 0
done:
%short = trunc i32 %r to i1
ret %short
}
func @main() {
entry:
%q = qalloc 1
%out = qalloc 1
h %q
%m = mark
%v = call @work(%q)
rewind %m keep (%v -> %out)
h %q
ret
}
";

#[test]
fn unaligned_jump_records_and_discarded_allocations_live_in_the_worlds_overlay() {
    let p = bennett_ir::load(JUMPS).expect("jump program");
    let model = Model::new(&p, "main");
    let reference = model.run_joined(basis(model.initial("main", &[])), 1000);
    let mut e = Executor::new(&p, p.function_named("main").expect("main"), &[], options()).expect("start");
    let start = e.snapshot();
    audit(&p, &mut e);
    assert_eq!(e.outcome(), Some(Outcome::Returned(None)));
    assert_worlds(&model, &reference, &e.snapshot());
    assert_eq!(e.snapshot().classical.memory, start.classical.memory);
    e.reverse().expect("reverse");
    assert_eq!(e.snapshot().classical, start.classical);
    assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-11));
}

/// Count the journal payload independently: one byte per record, sixteen per destroyed word;
/// the known mark/read side tables are two machine words and two optional ones (position, allocations, the
/// budget, the deadline) / two machine words plus a slot and flag.
/// The byte totals at the split are computed from preceding instructions, never from Stats itself.
#[test]
fn both_split_payload_counters_match_an_instruction_by_instruction_count() {
    let p = bennett_ir::load(
        "memory 512
func @id(i1 %x)->i1 {
entry:
ret %x
}
func @main() {
entry:
%q = qalloc 1
%r = qalloc 1
h %q
h %r
%m = mark
%a = call @id(%q)
store i8 99, 1
%b = call @id(%r)
rewind %m
ret
}",
    )
    .expect("valid stats program");
    let mut e = Executor::new(&p, p.function_named("main").expect("main"), &[], options()).expect("start");
    // At the first call: two allocations (17 each), two gates (1 each), a mark (17),
    // and its side table (two 64-bit fields and two optional ones). All of this is cloned once.
    let mark_metadata = 2 * std::mem::size_of::<u64>() + 2 * std::mem::size_of::<Option<u64>>();
    let first = 2 * 17 + 2 + 17 + mark_metadata;
    for _ in 0..5 {
        assert_eq!(e.step(), None);
    }
    assert_eq!(e.stats().journal_bytes, Some(first));
    assert_eq!(e.step(), None);
    assert_eq!(e.stats().split_journal_bytes, first as u64);
    assert_eq!(e.stats().split_memory_bytes, 0);
    // Each read has a Call record and one old argument word (17), a ReadEntry
    // (usize record, usize column, u32 slot, bool; padded to usize alignment), then the
    // Return (three words and one record =49) and Store (17). Both worlds clone this.
    let read_metadata = (2 * std::mem::size_of::<usize>() + 5).next_multiple_of(std::mem::align_of::<usize>());
    let second = first + 17 + read_metadata + 49 + 17;
    assert_eq!(e.step(), None);
    assert_eq!(e.step(), None);
    assert_eq!(e.stats().journal_bytes, Some(2 * second));
    assert_eq!(e.step(), None);
    assert_eq!(e.stats().split_journal_bytes, (first + 2 * second) as u64);
    assert_eq!(e.stats().split_memory_bytes, 2);
    assert_eq!(e.stats().overlay_bytes, 4);
    assert_eq!(e.stats().peak_overlay_bytes, 4);
    assert_eq!(e.run(), Outcome::Returned(None));
    assert_eq!(e.stats().overlay_bytes, 0);
    println!("split journal {} + 2*{} = {}; split overlay = 2 bytes, peak = 4", first, second, first + 2 * second);
}

#[test]
fn a_store_crossing_the_memory_end_changes_no_byte_at_any_width() {
    for width in 1_u32..=128 {
        let address = 513 - width.div_ceil(8);
        let text = format!(
            "memory 512\ndata 0 = i64 1234605616436508552, zero 32\nfunc @work(i1 %x) {{\nentry:\nstore i{width} 1, {address}\nret\n}}\nfunc @main() {{\nentry:\n%q = qalloc 1\nh %q\ncall @work(%q)\nret\n}}\n"
        );
        let p = bennett_ir::load(&text).expect("valid boundary program");
        let mut e = Executor::new(&p, p.function_named("main").expect("main"), &[], options()).expect("start");
        let start = e.snapshot();
        for _ in 0..3 {
            assert_eq!(e.step(), None);
        }
        assert_eq!(e.stats().worlds, 2);
        assert_eq!(
            e.step(),
            Some(Outcome::Stopped(bennett_exec::RunError::WorldFault {
                fault: bennett_ir::Fault::OutOfBounds,
                worlds: 2
            })),
            "width {width}"
        );
        assert!(worlds(&e.snapshot()).all(|w| w.classical.memory == start.classical.memory));
        assert_eq!(e.stats().overlay_bytes, 0);
        e.reverse().expect("reverse boundary fault");
        assert_eq!(e.snapshot().classical, start.classical);
    }
    println!("128 memory-end crossings fault atomically in worlds and reverse");
}
