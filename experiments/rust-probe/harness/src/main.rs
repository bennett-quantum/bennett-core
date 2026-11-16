//! Exhaustive native comparison and whole-state reversal, outside the fast suite.
use bennett_exec::{Executor, Options, Outcome};
use std::{env, fs};

fn main() {
    let args: Vec<_> = env::args().collect();
    let module = bennett_ir::load(&fs::read_to_string(&args[1]).expect("device text")).expect("valid device IR");
    let native = fs::read(&args[2]).expect("native oracle words");
    let mut offset = 0;
    println!("function\tinputs\treturned\tfaulted\treversed\tsteps_total\tsteps_min\tsteps_max\tjournal_peak");
    for name in ["arithmetic", "branchy", "counted_loop", "checked", "ordinary_add"] {
        let unary = matches!(name, "branchy" | "counted_loop");
        let count = if unary { 256 } else { 65536 };
        let expected = &native[offset..offset + count * 2];
        offset += count * 2;
        let Some(entry) = module.function_named(name) else {
            assert_eq!(name, "ordinary_add", "unexpected missing entry");
            assert_eq!(args.get(3).map(String::as_str), Some("safe-only"));
            continue;
        };
        let mut total = 0;
        let mut min = u64::MAX;
        let mut max = 0;
        let mut journal_peak = 0;
        let mut returned = 0;
        let mut faulted = 0;
        for (index, word) in expected.chunks_exact(2).enumerate() {
            let want = u16::from_le_bytes([word[0], word[1]]);
            let (a, b) = if unary { (index as u64, 0) } else { ((index / 256) as u64, (index % 256) as u64) };
            let values = if unary { vec![a] } else { vec![a, b] };
            let mut exec = Executor::new(&module, entry, &values, Options {
                reversible: true,
                budget: Some(100_000),
                ..Options::default()
            }).expect("start");
            let before = exec.snapshot();
            let got = exec.run();
            if want == 256 {
                assert_eq!(got, Outcome::Faulted(bennett_ir::Fault::Unreachable), "{name}({a},{b})");
                faulted += 1;
            } else {
                assert_eq!(got, Outcome::Returned(Some(want as u128)), "{name}({a},{b})");
                returned += 1;
            }
            let steps = exec.stats().steps;
            total += steps;
            min = min.min(steps);
            max = max.max(steps);
            journal_peak = journal_peak.max(exec.stats().journal_bytes.unwrap_or_default());
            exec.reverse().expect("reverse");
            assert_eq!(exec.snapshot(), before, "full start state {name}({a},{b})");
        }
        println!("{name}\t{count}\t{returned}\t{faulted}\t{count}\t{total}\t{min}\t{max}\t{journal_peak}");
    }
    assert_eq!(offset, native.len(), "complete native oracle");
}
