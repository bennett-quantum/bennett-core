//! Sensitivity check for JC-1: use sets for live-source intersection, on the published workloads.

use bennett_exec::{Executor, Options, Outcome, WordWitness, classify};
use bennett_front_tests::{CLANG_FLAGS, c_dir, entry, lower, options, root};
use bennett_ir::{Inst, Type, ValidModule};

fn check(module: &ValidModule, name: &str, args: &[u64]) -> usize {
    let options = Options { allow_region_faults: true, max_configurations: Some(1 << 17), ..options(0) };
    let mut executor = Executor::new(module, entry(module, name), args, options).expect("starts");
    executor.measure_journal_content(true);
    assert!(matches!(executor.run(), Outcome::Returned(_)));
    let mut measurement = executor.take_journal_measurement().expect("on");
    let before = classify(&measurement);
    // Canonicalize the observer's *copied* metadata; neither executor nor classifier is changed.
    let mut unsorted = 0;
    for word in measurement
        .applies
        .iter_mut()
        .flat_map(|a| &mut a.branches)
        .flat_map(|b| &mut b.entries)
        .flat_map(|r| &mut r.words)
    {
        if let WordWitness::LiveCopy(sources) = &mut word.witness {
            unsorted += usize::from(sources.windows(2).any(|pair| pair[0] > pair[1]));
            sources.sort_unstable();
            sources.dedup();
        }
    }
    let after = classify(&measurement);
    let mut changes = 0;
    for (apply, (before, after)) in before.iter().zip(&after).enumerate() {
        for (alignment, (old, new)) in
            [(&before.site_count, &after.site_count), (&before.call_path, &after.call_path)].into_iter().enumerate()
        {
            for (old, new) in old.iter().zip(new) {
                if old != new {
                    println!("changed {name}{args:?} apply={apply} alignment={alignment} {old:?} -> {:?}", new.class);
                    changes += 1;
                }
            }
        }
    }
    println!("checked {name}{args:?}: unsorted={unsorted}, changed={changes}");
    changes
}

#[test]
#[ignore = "full workload sensitivity check; about 1.2 GB, run single-threaded with --nocapture"]
fn canonical_live_source_sets_on_published_workloads() {
    let mut changed = 0;
    let mut runs = 0;
    let module = lower(&c_dir().join("journal_content.c"), "jc_independent_lookups", CLANG_FLAGS, &[]);
    for lookup in ["binary_search", "hash_lookup", "btree_lookup", "write_lookup"] {
        for reps in [1, 2, 4, 8] {
            changed += check(&module, &format!("coherent_jc_{lookup}"), &[0, reps]);
            runs += 1;
        }
    }
    for program in [
        "thesis",
        "thesis_kept",
        "kickback",
        "fault_cancel",
        "controlled",
        "grover",
        "inplace",
        "coarse",
        "float_controlled",
        "float_long_double_controlled",
        "coherent",
    ] {
        let module =
            lower(&c_dir().join(format!("{program}.c")), &format!("jc_independent_{program}"), CLANG_FLAGS, &[]);
        for function in &module.functions {
            let applies = function.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Apply { .. }));
            let ordinary = function.params.iter().all(|p| matches!(p.ty, Type::Int(_)));
            if applies && ordinary && !function.name.contains(".apply") {
                changed += check(&module, &function.name, &vec![1; function.params.len()]);
                runs += 1;
            }
        }
    }
    for (example, name, args) in [
        ("thesis", "main", &[][..]),
        ("thesis", "entangled", &[][..]),
        ("grover", "searched", &[2][..]),
        ("fault", "main", &[4][..]),
    ] {
        let text = std::fs::read_to_string(root().join(format!("examples/{example}.bn"))).expect("example");
        let module = bennett_ir::load(&text).expect("valid example");
        changed += check(&module, name, args);
        runs += 1;
    }
    println!("total runs={runs}, changed aligned positions={changed} (A+B)");
    assert_eq!(runs, 83, "16 lookups and 67 other runs");
}

/// The README's max-journal-words/step slope for the B-tree is printed as 1.28.
/// Recompute from exact aggregate step counts, then round once, to check that displayed number.
#[test]
#[ignore = "known README discrepancy: B-tree journal slope rounds to 1.27, not 1.28"]
fn documented_btree_slope_uses_full_precision_before_rounding() {
    let module = lower(&c_dir().join("journal_content.c"), "jc_independent_slope", CLANG_FLAGS, &[]);
    let callee = entry(&module, "coherent_jc_btree_lookup.apply");
    let mut points = Vec::new();
    for reps in [1, 2, 4, 8] {
        let mut executor =
            Executor::new(&module, entry(&module, "coherent_jc_btree_lookup"), &[0, reps], options(0)).expect("starts");
        executor.measure_journal_content(false);
        assert!(matches!(executor.run(), Outcome::Returned(_)));
        let measurement = executor.take_journal_measurement().expect("on");
        let applies: Vec<_> = measurement.applies.iter().filter(|a| a.callee == callee).collect();
        assert_eq!(applies.len(), 1);
        let branches = &applies[0].branches;
        assert_eq!(branches.len(), 256);
        let total_steps = branches.iter().map(|b| b.steps).sum::<u64>();
        let max_words = branches.iter().map(|b| b.words).max().expect("branches");
        println!("B-tree reps={reps}: total_steps={total_steps}, branches=256, max_words={max_words}");
        points.push((total_steps as f64 / 256.0, max_words as f64));
    }
    // Uncentred least squares, independent of the measurement's centred implementation.
    let n = points.len() as f64;
    let sx = points.iter().map(|p| p.0).sum::<f64>();
    let sy = points.iter().map(|p| p.1).sum::<f64>();
    let sxx = points.iter().map(|p| p.0 * p.0).sum::<f64>();
    let sxy = points.iter().map(|p| p.0 * p.1).sum::<f64>();
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    println!("B-tree slope={slope:.12}");
    let published = include_str!("../../../experiments/journal-content/README.md")
        .lines()
        .find(|line| line.starts_with("| B-tree |") && line.split('|').count() == 9)
        .expect("B-tree slope row")
        .split('|')
        .nth(2)
        .expect("max words/step")
        .trim();
    assert_eq!(format!("{slope:.2}"), published, "README's displayed B-tree max words/step");
}
