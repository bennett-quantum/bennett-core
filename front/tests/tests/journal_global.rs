//! Global-write journal content. Heavy run: front/test.sh --test journal_global -- --ignored --nocapture --test-threads=1
use std::process::Command;
use std::sync::OnceLock;

use bennett_exec::{
    Executor, JournalMeasurement, Options, Outcome, Stats, WordClass, classify, classify_with_chains,
    classify_with_recompute,
};
use bennett_front_tests::{CLANG_FLAGS, c_dir, entry, lower, native_build, options};
use bennett_ir::{DataItem, DataSegment, ValidModule};

const WORK: [&str; 7] = [
    "jg_log",
    "jg_hash_insert",
    "jg_btree_insert",
    "jc_binary_search",
    "jc_hash_lookup",
    "jc_btree_lookup",
    "jc_write_lookup",
];

fn module(work: &str, bits: u32, second_split: bool, extra_memory: u64) -> ValidModule {
    static BASE: OnceLock<ValidModule> = OnceLock::new();
    let base = BASE.get_or_init(|| {
        let m = lower(&c_dir().join("journal_global.c"), "journal_global", CLANG_FLAGS, &["--entry=jg_setup"]);
        let mut e = Executor::new(&m, entry(&m, "jg_setup"), &[], options(0)).expect("setup");
        assert_eq!(e.run(), Outcome::Returned(None));
        let bytes = e.snapshot().classical.memory[..m.static_end() as usize].to_vec();
        let mut m = m.into_module();
        m.data = vec![DataSegment { address: 0, items: vec![DataItem::Bytes(bytes)] }];
        bennett_ir::validate(m).expect("initialized static image")
    });
    let mut m = base.clone().into_module();
    m.memory_size += extra_memory;
    let h: String = (0..bits).map(|i| format!("    h %q[{i}]\n")).collect();
    let prepare_r = if second_split { "    h %r\n" } else { "" };
    let split = if second_split { "    call @jg_id(%r)\n" } else { "" };
    let text = format!(
        "{}
func @jg_id(i1 %x) -> i1 {{
entry:
    ret %x
}}
func @world(i32 %reps) {{
entry:
    %q = qalloc 8
{h}    %out = qalloc 32
    %r = qalloc 1
{prepare_r}    %m = mark budget -1
    %v = call @{work}(%q, %reps)
{split}    rewind %m keep (%v -> %out)
    ret
}}
func @oracle(i32 %reps) {{
entry:
    %q = qalloc 8
{h}    %out = qalloc 32
    %r = qalloc 1
{prepare_r}    %fault = qalloc 5
    apply @{work}(%q, %reps) -> %out, %fault budget 100000
    ret
}}",
        bennett_ir::write(&m)
    );
    bennett_ir::load(&text).expect("handwritten driver")
}

fn opts() -> Options {
    Options { reversible: true, ..options(0) }
}

fn run_world(m: &ValidModule, reps: u64, observe: bool) -> (bennett_exec::Snapshot, Stats, usize) {
    let mut e = Executor::new(m, entry(m, "world"), &[reps], opts()).expect("world");
    let initial = e.snapshot();
    if observe {
        e.measure_journal_content(true);
    }
    let mut peak_journal = 0;
    loop {
        let done = e.step();
        peak_journal = peak_journal.max(e.stats().journal_bytes.expect("journal"));
        if let Some(outcome) = done {
            assert_eq!(outcome, Outcome::Returned(None));
            break;
        }
    }
    let stats = e.stats();
    let final_state = e.snapshot();
    assert_eq!(final_state.classical.memory, initial.classical.memory, "rewind restores global and stack bytes");
    assert_eq!((stats.worlds, stats.overlay_bytes, stats.observations), (1, 0, 0));
    if observe {
        assert!(
            e.take_journal_measurement().expect("observer").applies.is_empty(),
            "current hook does not observe ordinary worlds"
        );
    }
    e.reverse().expect("full reversal");
    let restored = e.snapshot();
    assert_eq!(restored.classical, initial.classical);
    assert!(restored.quantum.distance(&initial.quantum).expect("registers") < 1e-12);
    assert!(restored.worlds.is_empty());
    (final_state, stats, peak_journal)
}

fn oracle(m: &ValidModule, reps: u64, observe: bool) -> (bennett_exec::Snapshot, Stats, Option<JournalMeasurement>) {
    let mut e = Executor::new(m, entry(m, "oracle"), &[reps], opts()).expect("oracle");
    if observe {
        e.measure_journal_content(true);
    }
    assert_eq!(e.run(), Outcome::Returned(None));
    (e.snapshot(), e.stats(), e.take_journal_measurement())
}

fn check_state(world: &bennett_exec::Snapshot, oracle: &bennett_exec::Snapshot) {
    assert!(world.worlds.is_empty() && oracle.worlds.is_empty());
    assert_eq!(world.classical.memory, oracle.classical.memory);
    for (config, amp) in &world.quantum.amplitudes {
        let mut other = config.clone();
        other.push(0); // apply's extra fault register
        assert!((*amp - oracle.quantum.amplitude(&other)).norm() < 1e-12);
    }
    assert_eq!(world.quantum.amplitudes.len(), oracle.quantum.amplitudes.len());
}

#[test]
fn global_drivers_restore_memory_and_observation_changes_nothing() {
    for work in WORK {
        let m = module(work, 2, true, 0);
        let (a, sa, ja) = run_world(&m, 2, false);
        let (b, sb, jb) = run_world(&m, 2, true);
        assert_eq!((a.clone(), sa, ja), (b, sb, jb));
        let (o, so, _) = oracle(&m, 2, false);
        let (p, sp, measurement) = oracle(&m, 2, true);
        // With the observer on, `apply` is computed by branch enumeration (the observer classifies branches),
        // so only `region_cost`, which reports how the region was computed, may differ.
        let (mut so, mut sp) = (so, sp);
        so.region_cost = Default::default();
        sp.region_cost = Default::default();
        assert_eq!((o.clone(), so), (p, sp));
        let measured = measurement.expect("observed");
        assert_eq!(measured.applies[0].branches.len(), 8);
        if work == "jg_hash_insert" {
            for branch in &measured.applies[0].branches {
                assert_eq!(
                    branch.bytes_written_below(m.static_end()),
                    16,
                    "both keys and values must survive lowering"
                );
            }
        }
        check_state(&a, &o);
    }
}

#[test]
#[ignore = "overlay copy/memory-size sweep: front/test.sh --test journal_global overlay_copy_tables -- --ignored --nocapture"]
fn overlay_copy_tables() {
    println!(
        "overlay\twork\treps\textra_memory\tpeak_worlds\tpeak_overlay\tcopied_overlay\tcopied_journal\tpeak_journal\trewound"
    );
    for work in &WORK[..3] {
        for reps in [1, 2, 4, 8] {
            let mut reference = None;
            for extra in [0, 1 << 20, 4 << 20] {
                let m = module(work, 2, true, extra);
                let (_, stats, peak) = run_world(&m, reps, false);
                assert_eq!((stats.splits, stats.merges), (7, 7));
                // Unequal C paths can merge one pair before the last world takes its second read.
                assert!((4..=8).contains(&stats.peak_worlds));
                assert!(stats.split_memory_bytes > 0);
                if let Some(previous) = reference {
                    assert_eq!((stats, peak), previous, "cost counters independent of classical memory size");
                }
                reference = Some((stats, peak));
                println!(
                    "overlay\t{work}\t{reps}\t{extra}\t{}\t{}\t{}\t{}\t{peak}\t{}",
                    stats.peak_worlds,
                    stats.peak_overlay_bytes,
                    stats.split_memory_bytes,
                    stats.split_journal_bytes,
                    stats.rewound_steps
                );
            }
        }
    }
}

fn class_index(class: WordClass) -> usize {
    match class {
        WordClass::Same => 0,
        WordClass::Recoverable => 1,
        WordClass::Recomputed => 2,
        WordClass::Irreducible => 3,
    }
}

fn count(positions: &[bennett_exec::ClassifiedPosition]) -> [u64; 4] {
    let mut counts = [0; 4];
    for p in positions {
        counts[class_index(p.class)] += 1;
    }
    counts
}

#[test]
#[ignore = "256-branch C journal classification and worlds costs; command in module documentation"]
fn global_journal_tables() {
    let executable = native_build(&c_dir().join("journal_global.c"), "journal_global", &["-DJG_NATIVE"]);
    println!(
        "global\twork\treps\talign\tmode\tmean_steps\tmean_global_bytes\tmean_all_bytes\tmax_words\tsame\tcopy\trecomputed\tirreducible\tevaluations\tmax_depth\tstore_irreducible\tsame_instances\tcopy_instances\trecomputed_instances\tirreducible_instances\tcontrol_only"
    );
    println!(
        "world\twork\treps\tsteps\tworld_steps\trewound\trerun\tpeak_worlds\tpeak_journal_bytes\tpeak_overlay\tcopied_overlay\tcopied_journal\tend_journal"
    );
    for (which, work) in WORK.into_iter().enumerate() {
        for reps in [1, 2, 4, 8] {
            let m = module(work, 8, false, 0);
            let (state, stats, peak) = run_world(&m, reps, false);
            let (o, _, measurement) = oracle(&m, reps, true);
            check_state(&state, &o);
            let measured = measurement.expect("observed");
            let apply = &measured.applies[0];
            assert_eq!(apply.branches.len(), 256);
            for branch in &apply.branches {
                let x = branch.args[0];
                let output = Command::new(&executable)
                    .args([which.to_string(), x.to_string(), reps.to_string()])
                    .output()
                    .expect("native");
                assert!(output.status.success());
                let native: u128 = String::from_utf8(output.stdout).expect("UTF8").trim().parse().expect("integer");
                assert_eq!(branch.outcome, Outcome::Returned(Some(native)), "{work}({x},{reps})");
            }
            let n = apply.branches.len() as f64;
            let steps = apply.branches.iter().map(|b| b.steps).sum::<u64>() as f64 / n;
            let bytes = apply.branches.iter().map(|b| b.bytes_written_below(m.static_end())).sum::<u64>() as f64 / n;
            let all_bytes = apply.branches.iter().map(|b| b.bytes_written_below(m.memory_size)).sum::<u64>() as f64 / n;
            let words = apply.branches.iter().map(|b| b.words).max().expect("branches");
            for (mode, classification) in [
                ("copy", classify(&measured)),
                ("one", classify_with_recompute(&measured)),
                ("live", classify_with_chains(&measured, false)),
                ("tape", classify_with_chains(&measured, true)),
            ] {
                let c = &classification[0];
                for (align, positions) in [("A", &c.site_count), ("B", &c.call_path)] {
                    let [same, copy, recomputed, irreducible] = count(positions);
                    let mut instances = [0; 4];
                    for p in positions {
                        instances[class_index(p.class)] += p.instances;
                    }
                    assert_eq!(instances.iter().sum::<usize>(), apply.branches.iter().map(|b| b.words).sum::<usize>());
                    let [si, ci, ri, ii] = instances;
                    let control =
                        positions.iter().filter(|p| p.class == WordClass::Irreducible && p.same_where_present).count();
                    let evaluations: u64 = positions
                        .iter()
                        .filter_map(|p| {
                            p.chain.as_ref().filter(|c| c.recovered).map(|c| c.instructions * p.instances as u64)
                        })
                        .sum();
                    let depth = positions.iter().filter_map(|p| p.chain.as_ref().map(|c| c.depth)).max().unwrap_or(0);
                    let store_i = positions
                        .iter()
                        .filter(|p| p.role == bennett_exec::WordRole::Store && p.class == WordClass::Irreducible)
                        .count();
                    println!(
                        "global\t{work}\t{reps}\t{align}\t{mode}\t{steps:.6}\t{bytes:.6}\t{all_bytes:.6}\t{words}\t{same}\t{copy}\t{recomputed}\t{irreducible}\t{evaluations}\t{depth}\t{store_i}\t{si}\t{ci}\t{ri}\t{ii}\t{control}"
                    );
                }
            }
            println!(
                "world\t{work}\t{reps}\t{}\t{}\t{}\t{}\t{}\t{peak}\t{}\t{}\t{}\t{}",
                stats.steps,
                stats.world_steps,
                stats.rewound_steps,
                stats.rerun_steps,
                stats.peak_worlds,
                stats.peak_overlay_bytes,
                stats.split_memory_bytes,
                stats.split_journal_bytes,
                stats.journal_bytes.expect("journal")
            );
        }
    }
}
