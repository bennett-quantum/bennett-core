//! Targeted differential checks for memory reads and exchanges under qubit control.

use std::path::Path;

use bennett_exec::{Outcome, RunError};
use bennett_front_tests::{
    lower_text, lower_text_with_budget, lower_text_with_diagnostics, native_build, run_with_stats, write_c,
};
use bennett_ir::Fault;

use super::state::{self, Check, Field};
use super::{Case, Expectation, MODES, check_refused, compare};

fn phased_case(body: &str, trips: u64, shifts: &[u32]) -> Case {
    let mut check = Check::new(shifts.iter().map(|&shift| Field { packet: 0, shift }).collect());
    check.preparation.s = 2;
    Case { state: Some(check), ..Case::values(body.to_owned(), 2, trips, 1, true) }
}

fn check_all_modes(
    name: &str,
    case: &Case,
    check: impl Fn(&str, &[&str], &[&str], &Path, &[Vec<u128>]) -> Result<(), String>,
) {
    let source = write_c(name, &case.source());
    let native = native_build(&source, name, &["-DNATIVE"]);
    let table = case.native_table(&native);
    let failures: Vec<_> = MODES
        .iter()
        .filter_map(|(mode, cflags, flags)| {
            let name = format!("{name}-{mode}");
            check(&name, cflags, flags, &source, &table).err().map(|error| format!("{name}: {error}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn check_phased(name: &str, case: &Case) {
    check_all_modes(name, case, |name, cflags, flags, source, table| {
        let text = lower_text(source, name, cflags, flags)?;
        let module = bennett_ir::load(&text).map_err(|error| error.to_string())?;
        compare(case, &module, table, &mut 0)?;
        state::compare(
            case.state.as_ref().expect("state check"),
            &module,
            table,
            case.trips,
            0,
            &mut state::Counts::default(),
        )
    });
}

#[test]
fn merged_store_reads_through_a_fixed_alias_before_the_exchange() {
    let case = phased_case(
        r#"
static uint8_t a = 43, b = 9;
__attribute__((noinline)) static void set(uint8_t *p, unsigned n) { *p = n; }
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t *alias = &a;
    set(alias, n);
    uint8_t q = PREPARE(prep);
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    if (q & 1U) { a = *alias ^ q; b += q; }
    else a = 5;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    packet w = bennett_measure_bytes(&b, 1);
    return key | (v << 8) | (w << 16);
}
"#,
        71,
        &[0, 8, 16],
    );
    check_phased("held-fixed-alias", &case);
}

#[test]
fn outlined_read_precedes_a_store_through_an_alias() {
    let case = phased_case(
        r#"
uint8_t a = 43, b = 9;
__attribute__((noinline)) static void set(uint8_t *p, unsigned n) { *p = n; }
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep);
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    unsigned value;
    if (q & 1U) { value = a ^ q; b += q; }
    else value = 5;
    set(&a, n);
    a = value;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    packet w = bennett_measure_bytes(&b, 1);
    return key | (v << 8) | (w << 16);
}
"#,
        71,
        &[0, 8, 16],
    );
    check_phased("held-aliased-write-after-read", &case);
}

#[test]
fn volatile_read_under_qubit_control_is_refused_by_name() {
    check_refused(
        "held-volatile-read",
        r#"
volatile uint8_t a = 43;
uint8_t b = 9, c = 7;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep);
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    if (q & 1U) { c = a ^ q; b += q; }
    else c = 5;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&b, 1);
    packet w = bennett_measure_bytes(&c, 1);
    return key | (v << 8) | (w << 16);
}
"#,
        "a volatile or atomic load controlled by a value that depends on a qubit",
    );
}

#[test]
#[ignore = "fixed-address load faults even when no prepared value reaches the read"]
fn untaken_read_of_bytes_an_earlier_call_left_holding_qubits() {
    let mut case = phased_case(
        r#"
uint8_t source = 43;
__attribute__((noinline)) static void hold(void) {
    uint8_t r = 0;
    XBIT(r, 0);
    if (r) source += r;
    XBIT(r, 0);
}
packet entry(unsigned prep, unsigned n, unsigned column) {
    hold();
    uint8_t q = PREPARE(prep), x = 0;
    HBIT(q, 0); SBIT(q, 0);
    if (q & 2U) x = source / (q + 1U);
    else x = 5;
    packet key = bennett_measure(&q);
    packet value = bennett_measure(&x);
    packet bytes = bennett_measure_bytes(&source, 1);
    return key | (value << 8) | (bytes << 16);
}
"#,
        0,
        &[0, 8, 16],
    );
    case.qubits = 1;
    case.state.as_mut().expect("state check").preparation.s = 1;
    check_phased("held-untaken-read", &case);
}

#[test]
fn nested_exchanges_read_both_old_values_in_one_group() {
    let case = phased_case(
        r#"
uint16_t a = 3, b = 5;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    a = 3U * q + 7U;
    b = 5U * q + 9U;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned i = q; i != 0; --i) {
        #pragma clang loop unroll(disable)
        for (unsigned j = i; j != 0; --j) {
            a = a + 3U*b + i;
            b = b + 5U*a + j;
        }
    }
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    packet w = bennett_measure_bytes(&b, 2);
    return key | (v << 8) | (w << 24);
}
"#,
        0,
        &[0, 8, 24],
    );
    check_phased("held-nested-exchange", &case);
}

#[test]
fn a_later_exchange_clears_through_three_regions() {
    let case = phased_case(
        r#"
uint8_t a = 3;
uint16_t b = 17;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    if (q & 1U) { a += q; b += 3U * q; }
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) a = a * 3U + t;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) a = a * 5U + t;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 1);
    packet w = bennett_measure_bytes(&b, 2);
    return key | (v << 8) | (w << 16);
}
"#,
        0,
        &[0, 8, 16],
    );
    check_phased("held-three-regions", &case);
}

#[test]
fn exchanged_whole_and_overlapping_byte_share_one_old_value() {
    let case = phased_case(
        r#"
uint16_t a = 365;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    a = 129U * q + 257U;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) {
        a = a * 3U + t;
        ((uint8_t *)&a)[1] += t;
    }
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    return key | (v << 8);
}
"#,
        0,
        &[0, 8],
    );
    check_phased("held-overlapping-byte", &case);
}

#[test]
fn exchanged_adjacent_elements_share_one_shadow() {
    let case = phased_case(
        r#"
uint8_t a[2] = {3, 5};
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    a[0] = 3U * q + 7U;
    a[1] = 5U * q + 9U;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) {
        a[0] = a[0] * 3U + a[1];
        a[1] = a[1] * 5U + a[0];
    }
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(a, 2);
    return key | (v << 8);
}
"#,
        0,
        &[0, 8],
    );
    check_phased("held-adjacent-elements", &case);
}

#[test]
fn old_value_uses_the_earlier_value_of_another_exchange_member() {
    let case = phased_case(
        r#"
uint16_t a = 3, b = 5;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    a = 3U * q + 7U;
    ZBIT(barrier, 0);
    b = a * 7U + 1U;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) {
        a = a * 3U + b;
        b = b * 5U + a;
    }
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    packet w = bennett_measure_bytes(&b, 2);
    return key | (v << 8) | (w << 24);
}
"#,
        0,
        &[0, 8, 24],
    );
    check_phased("held-interdependent-exchange", &case);
}

#[test]
fn old_value_uses_a_variable_a_later_region_overwrote() {
    let case = phased_case(
        r#"
uint16_t a = 3, b = 5;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    a = 3U * q + 7U;
    ZBIT(barrier, 0);
    b = a * 7U + 1U;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) a = a * 3U + t;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) b = b * 5U + t;
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    packet w = bennett_measure_bytes(&b, 2);
    return key | (v << 8) | (w << 24);
}
"#,
        0,
        &[0, 8, 24],
    );
    check_phased("held-source-overwritten", &case);
}

#[test]
fn a_retained_loop_releases_one_old_value_with_zero_or_many_trips() {
    let mut case = phased_case(
        r#"
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), x = 0;
    HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    x = 4U + (q & 1U);
    XBIT(x, 1);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) x = x * 3U + t;
    packet key = bennett_measure(&q);
    packet value = bennett_measure(&x);
    return key | (value << 8);
}
"#,
        0,
        &[0, 8],
    );
    case.expectation = Expectation::LoopRetained { object: "x", bits: 8 };
    case.state.as_mut().expect("state check").retained = Some(vec![6, 7, 6, 7]);
    check_all_modes("held-retained-trips", &case, |name, cflags, flags, source, table| {
        let (text, diagnostics) = lower_text_with_diagnostics(source, name, cflags, flags)?;
        case.expectation.check_diagnostics(&diagnostics)?;
        let module = bennett_ir::load(&text).map_err(|error| error.to_string())?;
        compare(&case, &module, table, &mut 0)?;
        state::compare(case.state.as_ref().expect("state check"), &module, table, 0, 8, &mut state::Counts::default())
    });
}

#[test]
fn nested_exchange_budget_faults_for_only_the_longer_inputs() {
    let case = phased_case(
        r#"
uint16_t a = 3, b = 5;
packet entry(unsigned prep, unsigned n, unsigned column) {
    uint8_t q = PREPARE(prep), barrier = 0;
#ifndef NATIVE
    if (column) {
        if (prep & 1U) XBIT(q, 0);
        if (prep & 2U) XBIT(q, 1);
    } else {
        HBIT(q, 0); HBIT(q, 1); SBIT(q, 1);
    }
#endif
    a = 3U * q + 7U;
    b = 5U * q + 9U;
    ZBIT(barrier, 0);
    #pragma clang loop unroll(disable)
    for (unsigned i = 4U * q; i != 0; --i) {
        #pragma clang loop unroll(disable)
        for (unsigned j = i; j != 0; --j) {
            a = a + 3U*b + i;
            b = b + 5U*a + j;
        }
    }
    packet key = bennett_measure(&q);
    packet v = bennett_measure_bytes(&a, 2);
    packet w = bennett_measure_bytes(&b, 2);
    return key | (v << 8) | (w << 24);
}
"#,
        0,
        &[0, 8, 24],
    );
    check_phased("held-nested-budget-full", &case);
    check_all_modes("held-nested-budget-short", &case, |name, cflags, flags, source, table| {
        const BUDGET: u64 = 250;
        let text = lower_text_with_budget(source, name, cflags, flags, BUDGET)?;
        let module = bennett_ir::load(&text).map_err(|error| error.to_string())?;
        let check_budget = |input, column| run_with_stats(&module, "entry", &[input, 0, column], 0).0;
        let mut failed = 0;
        for (input, &expected) in table[0].iter().enumerate() {
            match check_budget(input as u64, 1) {
                Outcome::Returned(Some(value)) if value == expected => {}
                Outcome::Stopped(RunError::RegionFault {
                    fault: Fault::Budget,
                    faulting: 1,
                    configurations: 1,
                    ..
                }) => failed += 1,
                outcome => {
                    return Err(format!("input {input}: expected {expected:#x} or budget fault, got {outcome:?}"));
                }
            }
        }
        match check_budget(0, 0) {
            Outcome::Stopped(RunError::RegionFault { fault: Fault::Budget, faulting, configurations: 4, .. })
                if faulting == failed && (1..4).contains(&faulting) =>
            {
                Ok(())
            }
            outcome => Err(format!("expected budget fault on {failed} of four configurations, got {outcome:?}")),
        }
    });
}

#[test]
fn retained_word_counts_follow_the_number_of_executed_exchanges() {
    let body = r#"
__attribute__((noinline)) static unsigned one(unsigned prep) {
    uint8_t q = PREPARE(prep), x = 0;
    HBIT(q, 0); HBIT(q, 1);
    x = 4U + (q & 1U);
    XBIT(x, 1);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) x = x * 3U + t;
    return bennett_measure(&q) | ((unsigned)bennett_measure(&x) << 8);
}
packet entry(unsigned prep, unsigned n, unsigned column) {
    unsigned last = 0;
    #pragma clang loop unroll(disable)
    for (unsigned i = 0; i < n; ++i) last = one(prep);
    return last;
}
"#;
    for trips in [0, 1, 3] {
        let name = format!("held-retained-repeat-{trips}");
        let qubits = if trips == 0 { 0 } else { 2 };
        let case = Case::values(body.to_owned(), qubits, trips, 1, false);
        check_all_modes(&name, &case, |name, cflags, flags, source, table| {
            let (text, diagnostics) = lower_text_with_diagnostics(source, name, cflags, flags)?;
            let mut warnings = diagnostics.lines().filter(|line| line.contains("warning: the overwrite"));
            let expected = warnings.next().is_some_and(|line| {
                line.contains("of qubit variable 'x' in a loop")
                    && line.contains("keeps the old value in 8 qubits each time it runs")
            });
            if !expected || warnings.next().is_some() {
                return Err(format!("expected one warning of eight qubits per exchange, got {diagnostics:?}"));
            }
            let module = bennett_ir::load(&text).map_err(|error| error.to_string())?;
            compare(&case, &module, table, &mut 0)?;
            let (_, stats) = run_with_stats(&module, "entry", &[0, trips, 0], 0);
            if (stats.nonzero_releases, u64::from(stats.leftover_qubits)) != (trips, 8 * trips) {
                return Err(format!(
                    "{trips} exchanges: {} nonzero releases, {} qubits kept",
                    stats.nonzero_releases, stats.leftover_qubits,
                ));
            }
            Ok(())
        });
    }
}

#[test]
fn exchanges_through_pointers_that_may_overlap_are_refused_by_name() {
    check_refused(
        "held-possible-alias",
        r#"
uint8_t a = 3, b = 5;
__attribute__((noinline)) void update(uint8_t *p, uint8_t *r, unsigned prep) {
    uint8_t q = PREPARE(prep);
    HBIT(q, 0); HBIT(q, 1);
    #pragma clang loop unroll(disable)
    for (unsigned t = q; t != 0; --t) {
        *p = *p * 3U + *r;
        *r = *r * 5U + *p;
    }
    bennett_measure(&q);
}
packet entry(unsigned prep, unsigned n, unsigned column) {
    update(&a, n ? &b : &a, prep);
    packet first = bennett_measure_bytes(&a, 1);
    packet second = bennett_measure_bytes(&b, 1);
    return first | (second << 8);
}
"#,
        "which may reach the same bytes",
    );
}
