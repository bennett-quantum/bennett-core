//! Running again is charged to the budget of a call with a fault value, as it is to the run's budget and to an
//! applied function's region budget (the maintainer's decisions of the afternoon of 2026-11-16: running again
//! is charged to the budget; the call takes the region's rules, and `apply` equals mark, this call and
//! rewind-keep). Until 2026-11-16 the call's budget counted the steps taken but not the steps a rewind inside
//! it ran again, so a callee that rewinds one mark in a loop ran exponentially many steps again before its
//! budget ended it, where the same callee under `apply` (or as the whole run under a run budget) stops early.

mod common;

use bennett_exec::Outcome;
use bennett_ir::Fault;
use common::{bounded, load};

/// The steps and the replay charge of `mark; loop: rewind; br loop` within a budget, worked by hand: the n-th
/// rewind runs again C(n) steps, and a rewind whose running again would pass the budget (its own step counted
/// first) ends the loop. The same hand count as `history_independent.rs` for the run's budget.
fn loop_of_rewinds_by_hand(budget: u64) -> (u64, u64) {
    let mut c: Vec<u64> = vec![0, 0];
    for n in 2..64 {
        let w = |k: usize| k as u64 + (1..k).map(|j| 1 + c[j]).sum::<u64>();
        let charge = (1..n).map(w).sum();
        c.push(charge);
    }
    let (mut steps, mut charged) = (0, 0);
    if budget == 0 {
        return (0, 0);
    }
    steps += 1; // mark
    for &charge in &c[1..] {
        if steps + charged >= budget {
            break; // the br
        }
        steps += 1;
        if steps + charged >= budget || (charge > 0 && steps + charged + charge + 1 > budget) {
            break; // the rewind
        }
        (steps, charged) = (steps + 1, charged + charge);
    }
    (steps, charged)
}

#[test]
fn running_again_is_charged_to_the_calls_budget() {
    for budget in 1..=40u64 {
        let module = load(&format!(
            "memory 256
func @f() {{
entry:
    %h = mark
    br loop
loop:
    rewind %h
    br loop
}}
func @main() -> i5 {{
entry:
    %c = call @f() budget {budget}
    ret %c
}}"
        ));
        let (outcome, e) = common::run(&module, "main", &[], bounded());
        assert_eq!(outcome, Outcome::Returned(Some(Fault::Budget.code().into())), "budget {budget}");
        let (_, charged) = loop_of_rewinds_by_hand(budget);
        assert_eq!(e.stats().replay_charged, charged, "budget {budget}: steps run again, charged");
    }
}
