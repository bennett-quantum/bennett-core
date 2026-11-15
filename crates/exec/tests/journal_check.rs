//! Independent checks of the journal-content observer, kept as regression tests.

mod common;

use bennett_exec::{Executor, Outcome, SitePos, Undo, WordClass, WordRole, WordWitness, classify};
use common::{bounded, entry, load};

// @main reserves 3 slots (24 bytes), @f 2 (16): its entry SP is 984.
// Both branches overwrite cell 0 twice. The second store destroys a copy of %v, still in the same live
// slot on BOTH branches. On b=1 it ALSO happens to equal both stack pointers. The extra copies
// must not invalidate the common %v witness. Every journal word can be enumerated by hand:
// Call [0], Slot [0], Store [0], Store [17 or 984], Finish []. Thus 4 positions:
// 3 Same and 1 Recoverable, under both alignments.
const COPY_WITH_EXTRA_MATCHES: &str = "memory 1024
func @f(i1 %b) -> i8 {
entry:
    %v = select i64 %b, 984, 17
    store i64 %v, 0
    store i64 7, 0
    ret 0
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 4
    apply @f(%q) -> %y, %e budget 20
    ret 0
}
";

#[test]
fn additional_matching_stack_pointers_do_not_erase_a_live_slot_witness() {
    let module = load(COPY_WITH_EXTRA_MATCHES);
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], bounded()).expect("starts");
    executor.measure_journal_content(true);
    assert_eq!(executor.run(), Outcome::Returned(Some(0)));
    let measurement = executor.take_journal_measurement().expect("on");
    let apply = &measurement.applies[0];
    let witnesses: Vec<_> = apply
        .branches
        .iter()
        .map(|branch| {
            assert_eq!((branch.steps, branch.records, branch.words), (4, 5, 4));
            branch
                .entries
                .iter()
                .find(|r| r.kind == Undo::Store && r.site.pos == SitePos::Inst(2))
                .expect("second store")
                .words[0]
                .clone()
        })
        .collect();
    assert_eq!(witnesses.iter().map(|w| w.value).collect::<Vec<_>>(), [17, 984]);
    let (WordWitness::LiveCopy(first), WordWitness::LiveCopy(second)) = (&witnesses[0].witness, &witnesses[1].witness)
    else {
        panic!("both instances have live copies: {witnesses:?}");
    };
    assert!(first.iter().any(|source| second.contains(source)), "a common live source exists");
    let classes = classify(&measurement);
    let observed: Vec<_> = [&classes[0].site_count, &classes[0].call_path]
        .into_iter()
        .map(|positions| {
            positions
                .iter()
                .find(|p| p.role == WordRole::Store && p.site.pos == SitePos::Inst(2))
                .expect("second store")
                .class
        })
        .collect();
    assert_eq!(observed, [WordClass::Recoverable; 2], "the same %v slot holds both old values under A and B");
}

/// Independent hand calculation covering every recovery rule. @f(0) journals
/// Call[0], Slot[0], Store[0], Jump[0], Finish[]. @f(1) journals that same prefix,
/// then Slot[0], Store[0], Store[3], Store[11], Jump[1], Finish[]. The four
/// shared words are Same; the four then-block writes recover by FirstWrite,
/// StoreBase, Unchanged and LiveCopy respectively; the last Jump is control-only.
#[test]
fn hand_enumeration_of_all_four_recovery_rules() {
    use bennett_exec::RecoverableReason;
    let module = load(
        "memory 1024
func @f(i1 %b) -> i8 {
entry:
    %v = select i8 %b, 11, 10
    store i8 %v, 0
    br %b, then, done
then:
    %fresh = add i8 %v, 1
    store i8 3, 8
    store i8 3, 8
    store i8 7, 0
    br done
done:
    ret 0
}
func @main() -> i8 {
entry:
    %q = qalloc 1
    h %q
    %y = qalloc 8
    %e = qalloc 4
    apply @f(%q) -> %y, %e budget 20
    ret 0
}",
    );
    let mut executor = Executor::new(&module, entry(&module, "main"), &[], bounded()).expect("starts");
    executor.measure_journal_content(true);
    assert_eq!(executor.run(), Outcome::Returned(Some(0)));
    let measurement = executor.take_journal_measurement().expect("on");
    let branches = &measurement.applies[0].branches;
    assert_eq!(branches.len(), 2);
    for (branch, (steps, values)) in branches.iter().zip([(4, vec![0, 0, 0, 0]), (9, vec![0, 0, 0, 0, 0, 0, 3, 11, 1])])
    {
        assert_eq!(branch.outcome, Outcome::Returned(Some(0)));
        assert_eq!((branch.steps, branch.records, branch.words), (steps, steps as usize + 1, values.len()));
        assert_eq!(branch.entries.iter().flat_map(|r| r.words.iter().map(|w| w.value)).collect::<Vec<_>>(), values);
    }
    let classifications = classify(&measurement);
    for positions in [&classifications[0].site_count, &classifications[0].call_path] {
        assert_eq!(positions.len(), 9);
        assert_eq!(positions.iter().filter(|p| p.class == WordClass::Same).count(), 4);
        let recovered: Vec<_> =
            positions.iter().filter(|p| p.class == WordClass::Recoverable).map(|p| (p.site.pos, p.reason)).collect();
        assert_eq!(
            recovered,
            [
                (SitePos::Inst(0), Some(RecoverableReason::FirstWrite)),
                (SitePos::Inst(1), Some(RecoverableReason::StoreBase)),
                (SitePos::Inst(2), Some(RecoverableReason::Unchanged)),
                (SitePos::Inst(3), Some(RecoverableReason::LiveCopy)),
            ]
        );
        let control: Vec<_> = positions.iter().filter(|p| p.class == WordClass::Irreducible).collect();
        assert_eq!(control.len(), 1);
        assert_eq!(
            (control[0].role, control[0].same_where_present, control[0].branches_present),
            (WordRole::JumpSource, true, 1)
        );
        assert_eq!(positions.iter().map(|p| p.instances).sum::<usize>(), 13);
    }
}
