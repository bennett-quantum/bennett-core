//! Tests of the documented sum and fault-group policy, with actual executed rewinds.
use super::*;
use crate::executor::Options;
use num_complex::Complex64 as C;
use std::collections::BTreeMap;

const FOUR: &str = "memory 1024\nfunc @main() {\nentry:\nz [64]\n%m = mark\n%first = load i8 64\nry(0.37) [64]\nry(1.1) [64:1]\nrx(0.91) [64:1] ctrl [64]\nry(0.63) [64] ctrl [64:1]\n%last = load i8 64\nrewind %m\nret\n}\n";

#[test]
fn four_executed_parts_require_more_than_pairwise_cancellation() {
    let p = bennett_ir::load(FOUR).expect("valid");
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("main"),
        &[],
        Options { reversible: true, ..Options::default() },
    )
    .expect("starts");
    for _ in 0..8 {
        assert_eq!(e.step(), None);
    }
    assert_eq!(e.worlds.len(), 4);
    // Execute precisely the next rewind of each world, delaying only the scheduler's check barrier.
    // This lets the independent arithmetic inspect all notes before the last one triggers the check.
    for i in 0..4 {
        e.select(i);
        assert!(e.next_is_rewind());
        assert!(e.execute().is_ok());
        e.status = Status::Parked;
    }
    let mut worlds = e.take_worlds();
    let mut parts = BTreeMap::<(u64, Vec<u64>), Vec<C>>::new();
    for w in &worlds {
        assert!(w.pending);
        for d in &w.deferred {
            for (c, a) in &d.amplitudes {
                parts.entry((d.key, c.to_vec())).or_default().push(*a);
            }
        }
    }
    let irreducible = parts
        .values()
        .filter(|v| {
            v.len() == 4
                && v.iter().copied().sum::<C>().norm() < 1e-12
                && (1..15).all(|mask| {
                    v.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, a)| *a).sum::<C>().norm() > 1e-8
                })
        })
        .count();
    assert!(irreducible > 0, "need four non-pair-cancelling contributions: {parts:?}");
    assert_eq!(e.make_deferred_checks(&mut worlds), None);
    assert!(worlds.iter().all(|w| !w.pending && w.backups.is_empty()));
    e.merge_equal(&mut worlds, |_| true);
    assert_eq!(worlds.len(), 1);
    let w = &worlds[0];
    assert_eq!(w.quantum.amplitudes().len(), 1);
    assert!((w.quantum.amplitudes().values().next().expect("amp") - C::new(1.0, 0.0)).norm() < 1e-12);
    println!("4 executed worlds: {irreducible} failing configurations cancel only on the full four-part sum");
}

#[test]
fn empty_parts_are_dropped_only_after_checks_and_failure_restores_the_entire_deepest_group() {
    let p = bennett_ir::load(FOUR).expect("valid");
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("main"),
        &[],
        Options { reversible: true, allow_region_faults: true, ..Options::default() },
    )
    .expect("starts");
    // Explicit policy domains: copies all come from an executed reachable state, then vary only the
    // check bookkeeping and coefficients. This checks the check barrier, not program reachability.
    assert_eq!(e.step(), None);
    let template = e.clone_world(None);
    let empty = template.quantum.with_amplitudes(Amplitudes::new());
    let failing = |a: f64| BTreeMap::from([(vec![1_u64].into_boxed_slice(), C::new(a, 0.0))]);
    let part = |a: f64| {
        let mut w = template.clone();
        w.status = Status::Parked;
        w.pending = true;
        w.backups.push(template.clone());
        w.quantum = empty.clone();
        w.deferred.push(Defect { key: 17, amplitudes: failing(a) });
        w
    };
    let mut pass = vec![part(1.0), part(2.0), part(-3.0)];
    assert_eq!(e.make_deferred_checks(&mut pass), None);
    assert!(pass.is_empty(), "an emptied share is removed once its notes cancel");
    let mut fail = vec![part(1.0), part(2.0), part(-2.0)];
    assert_eq!(e.make_deferred_checks(&mut fail), None);
    assert_eq!(fail.len(), 3);
    assert!(
        fail.iter()
            .all(|w| w.outcome == Some(Outcome::Faulted(Fault::BadRewind)) && !w.quantum.amplitudes().is_empty())
    );
    // A second group of shorter history is not restored or faulted by the first group's failure.
    let mut shorter = template.clone();
    shorter.status = Status::Parked;
    shorter.journal = Some(crate::journal::Journal::default());
    let mut fail = vec![part(1.0), part(0.0), shorter];
    e.allow_region_faults = false;
    assert_eq!(e.make_deferred_checks(&mut fail), Some(RunError::WorldFault { fault: Fault::BadRewind, worlds: 3 }));
    assert_eq!(fail.iter().filter(|w| w.outcome == Some(Outcome::Faulted(Fault::BadRewind))).count(), 2);
    assert_eq!(fail.iter().filter(|w| w.status == Status::Parked).count(), 1);
}

/// CARGO_BUILD_JOBS=3 cargo test -p bennett-exec --lib deferred_hash_key_sample -- --ignored --nocapture
#[test]
#[ignore = "measure one million actual deferred-check keys and the 64-bit birthday bound"]
fn deferred_hash_key_sample() {
    use std::collections::HashSet;
    use std::time::Instant;
    let text = FOUR.replace("func @main()", "func @main(i64 %state)");
    let p = bennett_ir::load(&text).expect("valid");
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("main"),
        &[0],
        Options { reversible: true, ..Options::default() },
    )
    .expect("starts");
    for _ in 0..3 {
        assert_eq!(e.step(), None);
    }
    e.deferring = true;
    let failing = BTreeMap::from([(vec![1_u64].into_boxed_slice(), C::new(0.25, 0.0))]);
    let n = 1_u64 << 20;
    let mut keys = HashSet::with_capacity(n as usize);
    let start = Instant::now();
    for i in 0..n {
        e.slots[0] = u128::from(i);
        e.defer(crate::journal::Undo::Slot, failing.clone());
        let d = e.deferred.pop().expect("one note");
        assert!(keys.insert(d.key), "actual distinct-state key collision at sample {i}");
    }
    let seconds = start.elapsed().as_secs_f64();
    let median = (2.0 * 2_f64.powi(64) * 2_f64.ln()).sqrt();
    println!(
        "{n} distinct actual check states, {seconds:.6}s ({:.0}/s), no collision; sample birthday probability {:.3e}; 50% collision {:.0} states / {:.1}s at measured rate / {:.1} GB for packed (key,input) pairs",
        n as f64 / seconds,
        (n as f64).powi(2) / 2_f64.powi(65),
        median,
        median * seconds / n as f64,
        median * 16.0 / 1e9
    );
}
