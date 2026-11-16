//! Direct whole-world interpretation of address gates; no register-program translation.
//! DESIGN.md "Qubits in memory": the first gate moves the byte into a cell, ordinary accesses
//! split per value and retain what they read/wrote, apply sees each configuration's private bytes,
//! and keeps need cells older than the mark. The reference implements those sentences directly.
//! Worlds' shared-history sentence requires checks on the sum, with the stated 1e-9 tolerance.
#[allow(dead_code, reason = "shared independent reference helpers")]
mod oracle_support;

use bennett_exec::{Executor, Options, Outcome};
use bennett_ir::Fault;
use oracle_support::worlds::{Finish, Model, basis};
use oracle_support::worlds_executor::{assert_worlds, observed_one_of};

fn compare(text: &str) -> bool {
    compare_seed(text, 0).0
}

fn compare_seed(text: &str, seed: u64) -> (bool, Outcome) {
    let p = bennett_ir::load(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let m = Model::new(&p, "main");
    let mut prefix = basis(m.initial("main", &[]));
    let mut prefix_exec = Executor::new(
        &p,
        p.function_named("main").expect("main"),
        &[],
        Options { reversible: true, allow_region_faults: true, ..Options::default() },
    )
    .expect("starts");
    // The independent step interpreter matches every pre-rewind whole-world snapshot.
    // Padding at a rewind belongs to the documented scheduler; completed regions compare below.
    for _ in 0..2000 {
        assert_worlds(&m, &prefix, &prefix_exec.snapshot());
        if prefix.keys().any(|w| {
            let f = w.frame();
            matches!(
                p.function(f.function).block(f.block).insts.get(f.instruction),
                Some(bennett_ir::Inst::Rewind { .. })
            )
        }) {
            break;
        }
        prefix = m.step(&prefix);
        prefix_exec.step();
    }
    let expected = m.run_joined(basis(m.initial("main", &[])), 2000);
    let refuses = expected.keys().any(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadRewind)));
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("main"),
        &[],
        Options { reversible: true, allow_region_faults: true, budget: Some(10000), seed, ..Options::default() },
    )
    .expect("starts");
    let start = e.snapshot();
    let out = e.run();
    assert!(
        expected.keys().any(|w| match (&w.classical.finish, out) {
            (Some(Finish::Returned(n)), Outcome::Returned(v)) => *n == v,
            (Some(Finish::Faulted(f)), Outcome::Faulted(g)) => *f == g,
            _ => false,
        }),
        "unexpected outcome {out:?}\n{text}"
    );
    let s = e.snapshot();
    if e.stats().observations == 0 {
        assert_worlds(&m, &expected, &s);
        e.reverse().expect("unobserved program reverses");
        assert_eq!(e.snapshot().classical, start.classical, "{text}");
        assert!(e.snapshot().quantum.distance(&start.quantum).is_some_and(|d| d < 1e-10));
    } else {
        assert!(observed_one_of(&m, &expected, &s), "sample not in full reference support\n{text}\n{s:?}");
    }
    (refuses, out)
}

fn generated(seed: usize, width: u32) -> String {
    let prep = [
        "h [64]\nh [65:7]",
        "ry(0.43) [64]\nrx(0.79) [65:7]",
        "h [64]\ns [64]\nh [65:7]",
        "h [64]\nx [65:7] ctrl [64]",
    ][seed % 4];
    let promotions: String = (0..width.div_ceil(8)).map(|i| format!("z [128:{bit}]\n", bit = 8 * i)).collect();
    let read: String = match (seed / 4) % 4 {
        0 => "%v = load i16 64\n".into(),
        1 => "store i9 257, 64\n%v = load i16 64\n".into(),
        2 => "%inner = mark budget -1\n%a = load i16 64\nstore i9 3, 64\nrewind %inner\n%v = load i16 64\n".into(),
        _ => "%a = load i16 64\n%p = and i16 %a, 1\n%p64 = zext i16 %p to i64\n%addr = add i64 %p64, 80\nstore i16 %a, %addr\n%v = load i16 64\n".into(),
    };
    let middle = [
        "",
        "h [65:7]",
        "x [64] ctrl %o[0]",
        "x %o[0] ctrl [64]",
        "ry(0.39) [64]\ns %o[1]",
        "z [66]\nh [66]\n%tb = load i8 66",
    ][(seed / 16) % 6];
    let keep = if seed & 128 == 0 { "%wide -> [128]" } else { "%b -> %o" };
    format!(
        "memory 2048\ndata 64 = i8 0x34, i8 0x52, i8 0xa7\nfunc @f(i16 %a)->i8 {{\nentry:\n%x = load i8 65\nstore i8 99, 65\n%r = xor i8 %x, 42\nret %r\n}}\nfunc @main() {{\nentry:\n%o = qalloc 8\n%fault = qalloc 5\n{promotions}{prep}\n%m = mark budget -1\n{read}%b = trunc i16 %v to i8\n%wide = zext i8 %b to i{width}\napply @f(%v) -> %o, %fault budget 100\n{middle}\nrewind %m keep ({keep})\nret\n}}\n"
    )
}

#[test]
fn mixed_addresses_accesses_regions_and_nested_marks_match_direct_reference() {
    let mut refused = 0;
    for seed in (0..256).step_by(17).chain([129]) {
        refused += usize::from(compare(&generated(seed, 16)));
    }
    println!("17 direct mixed-memory programs, {refused} refusals");
}

/// CARGO_BUILD_JOBS=3 cargo test -p bennett-exec --test memory_address_independent generated_memory_extended -- --ignored --nocapture
#[test]
#[ignore = "generated direct memory reference comparisons and all integer keep widths"]
fn generated_memory_extended() {
    let mut refused = 0;
    for seed in 0..256 {
        refused += usize::from(compare(&generated(seed, 16)));
    }
    for width in 1..=128 {
        // A cast to a smaller width must be a truncation.
        let text = generated(64 + width as usize % 4, width)
            .replace("zext i8 %b to i", if width < 8 { "trunc i8 %b to i" } else { "zext i8 %b to i" });
        let text = if width == 8 { text.replace("%wide = zext i8 %b to i8", "%wide = or i8 %b, 0") } else { text };
        refused += usize::from(compare(&text));
    }
    println!("384 direct memory programs, {refused} refusals");
}

#[test]
fn every_bit_offset_and_nonzero_first_gate_move_matches_direct_reference() {
    for bit in 0..128 {
        let text = format!(
            "memory 512\ndata 64 = i128 0x123456789abcdef00123456789abcdef\nfunc @main() {{\nentry:\n%o = qalloc 1\n%p = add i64 63, 1\nh [%p:{bit}]\n%m = mark budget -1\n%v = load i128 64\nh [%p:{bit}]\nrewind %m keep (i1 1 -> %o)\nh [%p:{bit}]\nret\n}}\n"
        );
        compare(&text);
    }
}

#[test]
fn jump_records_read_and_write_quantum_bytes_including_high_bits() {
    for byte in [0, 3, 7, 8, 15, 16, 23, 24, 31] {
        let text = format!(
            "memory 1024\ndata 64 = i128 0xa5a5, i128 0x1234\nfunc @throw(i64 %buf) {{\nentry:\nlongjmp %buf, 7\n}}\nfunc @main() {{\nentry:\n%out = qalloc 8\nh [{cell}:0]\n%m = mark budget -1\n%j = setjmp 64\n%c = icmp eq i32 %j, 0\nbr %c, first, done\nfirst:\ncall @throw(i64 64)\nunreachable:\nfault unreachable\ndone:\n%l = load i8 64\nrewind %m keep (i8 1 -> %out)\nret\n}}\n",
            cell = 64 + byte
        );
        // An unreachable label needs a terminator before it.
        compare(&text.replace("call @throw(i64 64)\nunreachable:", "call @throw(i64 64)\nbr done\nunreachable:"));
    }
}

fn staggered(refuses: bool) -> String {
    let keep = if refuses { " keep (i1 1 -> %o)" } else { "" };
    format!(
        "memory 1024\nfunc @slow(i2 %x)->i8 {{\nentry:\nbr loop(%x)\nloop(i2 %n):\n%c = icmp eq i2 %n, 0\nbr %c, done, more\nmore:\n%next = sub i2 %n, 1\nbr loop(%next)\ndone:\n%r = zext i2 %x to i8\nret %r\n}}\nfunc @main() {{\nentry:\n%o = qalloc 1\nz [64]\n%m = mark budget -1\n%first = load i8 64\nx [64] ctrl %o\nry(0.37) [64]\nry(1.1) [64:1]\nrx(0.91) [64:1] ctrl [64]\nry(0.63) [64] ctrl [64:1]\n%last = load i8 64\n%v = trunc i8 %last to i2\n%r = call @slow(%v)\nstore i8 %r, 80\nrewind %m{keep}\nret\n}}\n"
    )
}

#[test]
fn four_staggered_worlds_check_the_sum_and_reverse_at_every_budget_stop() {
    let mut stops = 0;
    let mut parked_stops = 0;
    for refuses in [false, true] {
        let text = staggered(refuses);
        assert_eq!(compare(&text), refuses);
        let p = bennett_ir::load(&text).expect("valid");
        let opts = Options { reversible: true, ..Options::default() };
        let entry = p.function_named("main").expect("main");
        let mut full = Executor::new(&p, entry, &[], opts).expect("starts");
        assert_eq!(full.run(), if refuses { Outcome::Faulted(Fault::BadRewind) } else { Outcome::Returned(None) });
        let ticks = full.stats().steps;
        for budget in 0..ticks {
            let mut e = Executor::new(&p, entry, &[], Options { budget: Some(budget), ..opts }).expect("starts");
            let start = e.snapshot();
            assert_eq!(e.run(), Outcome::Faulted(Fault::Budget), "budget {budget}");
            let s = e.snapshot();
            let parked = std::iter::once(&s)
                .chain(&s.worlds)
                .any(|w| w.classical.frames.len() == 1 && w.classical.frames[0].inst == 14);
            parked_stops += usize::from(parked && !s.worlds.is_empty());
            e.reverse().expect("budget before pending checks reverses");
            let end = e.snapshot();
            assert!(end.worlds.is_empty());
            assert_eq!(end.classical, start.classical);
            assert_eq!(end.quantum.cells, start.quantum.cells);
            assert!(end.quantum.distance(&start.quantum).is_some_and(|d| d < 1e-12));
            stops += 1;
        }
    }
    assert!(parked_stops > 0);
    println!("{stops} budget stops invert, {parked_stops} with a rewound part waiting for others");
}

#[test]
fn disagreeing_longjmp_record_is_the_documented_bad_longjmp_debt() {
    let p = bennett_ir::load("memory 512\nfunc @main() {\nentry:\n%j = setjmp 64\nh [64]\nlongjmp 64, 1\n}\n")
        .expect("valid");
    let m = Model::new(&p, "main");
    let expected = m.run_joined(basis(m.initial("main", &[])), 100);
    assert!(expected.keys().all(|w| w.classical.finish == Some(Finish::Faulted(Fault::BadLongjmp))));
    let mut e = Executor::new(
        &p,
        p.function_named("main").expect("main"),
        &[],
        Options { reversible: true, ..Options::default() },
    )
    .expect("starts");
    assert_eq!(e.run(), Outcome::Faulted(Fault::BadLongjmp));
    assert_worlds(&m, &expected, &e.snapshot());
    e.reverse().expect("fault reverses");
}

#[test]
fn nested_replay_failure_preserves_the_outer_rewind_state() {
    let text = "memory 512\nfunc @id(i1 %x)->i1 {\nentry:\nret %x\n}\nfunc @main() {\nentry:\n%q = qalloc 1\n%o = qalloc 1\n%r = qalloc 1\nry(1.1) %q\nrx(0.3) %o\n%m = mark budget -1\n%v1 = call @id(%q)\nx %q ctrl %o\n%n = mark budget -1\n%w = call @id(%q)\nh %q\nrewind %n keep (%w -> %r)\nrewind %m keep (%v1 -> %o)\nret\n}\n";
    compare(text);
    let memory = text
        .replace("%q = qalloc 1", "z [64]")
        .replace("%v1 = call @id(%q)", "%vb = load i8 64\n%v1 = trunc i8 %vb to i1")
        .replace("%w = call @id(%q)", "%wb = load i8 64\n%w = trunc i8 %wb to i1")
        .replace("%q", "[64]");
    compare(&memory);
}

#[test]
fn local_changed_read_faults_only_its_world_and_both_host_outcomes_match_reference() {
    let text = generated(129, 16);
    let (mut returned, mut faulted) = (0, 0);
    for seed in 0..128 {
        let (_, out) = compare_seed(&text, seed);
        match out {
            Outcome::Returned(None) => returned += 1,
            Outcome::Faulted(Fault::BadRewind) => faulted += 1,
            _ => panic!("unexpected mixed outcome {out:?}"),
        }
    }
    assert!(returned > 0 && faulted > 0);
    println!("128 mixed-fault host observations: {returned} returns / {faulted} bad_rewind, full reference support");
}
