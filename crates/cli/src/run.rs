//! `bennett run`: execute, optionally reverse, and report.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use bennett_exec::{Executor, Options, Outcome, ReverseError, Snapshot, Stats};
use bennett_ir::{Const, FuncId, Type, ValidModule};

use crate::{CliError, RunArgs};

pub(crate) fn run(args: &RunArgs) -> Result<ExitCode, CliError> {
    let module = crate::load(&args.file)?;
    let entry = module.function_named(&args.entry).ok_or_else(|| CliError::NoEntry(args.entry.clone()))?;
    let values = entry_arguments(&module, entry, &args.args)?;
    let options = Options {
        budget: args.budget,
        max_configurations: args.max_configurations,
        seed: args.seed,
        reversible: args.reverse,
    };
    let mut executor = Executor::new(&module, entry, &values, options)?;
    let entry_state = executor.snapshot();

    let started = Instant::now();
    let outcome = executor.run();
    let forward = Timed { steps: executor.stats().steps, time: started.elapsed() };
    let stats = executor.stats();
    report_outcome(&module, entry, outcome, executor.tape(), stats.steps);

    let reverse = args.reverse.then(|| reverse_and_report(&mut executor, &entry_state, &stats));
    if args.stats {
        report_stats(&stats, forward, reverse.flatten());
    }
    Ok(match outcome {
        Outcome::Returned(_) => ExitCode::SUCCESS,
        Outcome::Faulted(_) => ExitCode::FAILURE,
        Outcome::Stopped(_) => ExitCode::from(2),
    })
}

/// The entry arguments as canonical values of the parameter widths.
fn entry_arguments(module: &ValidModule, entry: FuncId, literals: &[i128]) -> Result<Vec<u64>, CliError> {
    let function = module.function(entry);
    if function.params.len() != literals.len() {
        let (expected, found) = (function.params.len(), literals.len());
        return Err(CliError::ArgumentCount { function: function.name.clone(), expected, found });
    }
    let convert = |(index, (param, &value)): (usize, (&bennett_ir::Param, &i128))| match param.ty {
        Type::Int(width) => Const::from_literal(width, value).map(Const::value).ok_or(CliError::ArgumentRange {
            index,
            value,
            ty: param.ty,
        }),
        // The executor rejects register parameters with its own error.
        Type::Qreg(_) => Ok(0),
    };
    function.params.iter().zip(literals).enumerate().map(convert).collect()
}

fn report_outcome(module: &ValidModule, entry: FuncId, outcome: Outcome, tape: &[u8], steps: u64) {
    match outcome {
        Outcome::Returned(None) => println!("result: none"),
        Outcome::Returned(Some(value)) => match module.function(entry).result {
            Some(Type::Int(width)) if width.bits() > 1 && value >> (width.bits() - 1) == 1 => {
                let signed = value as i64 | !(width.mask() as i64);
                println!("result: {value} (signed {signed})");
            }
            _ => println!("result: {value}"),
        },
        Outcome::Faulted(fault) => println!("fault: {fault} (code {})", fault.code()),
        Outcome::Stopped(error) => println!("stopped: {error}"),
    }
    if !tape.is_empty() {
        println!("console: \"{}\"", tape.escape_ascii());
    }
    println!("steps: {steps}");
}

/// Reverses the run and reports whether the state returned to `entry_state`; returns the timing.
fn reverse_and_report(executor: &mut Executor<'_>, entry_state: &Snapshot, stats: &Stats) -> Option<Timed> {
    if let (Some(bytes), Some(per_step)) = (stats.journal_bytes, per_step(stats)) {
        println!("journal: {bytes} bytes, {per_step:.1} bytes per step");
    }
    let started = Instant::now();
    match executor.reverse() {
        Ok(steps) => {
            let time = started.elapsed();
            let state = executor.snapshot();
            let classical = if state.classical == entry_state.classical { "identical" } else { "DIFFERENT" };
            match state.quantum.distance(&entry_state.quantum) {
                Some(distance) => println!(
                    "reverse: undid {steps} steps; returned to entry: classical state {classical}, \
                     amplitudes within {distance:e}"
                ),
                None => println!("reverse: undid {steps} steps; returned to entry: NO (registers differ)"),
            }
            Some(Timed { steps, time })
        }
        Err(error @ ReverseError::PastMeasurement) => {
            println!("reverse: stopped after undoing {} steps: {error}", stats.steps - executor.stats().steps);
            None
        }
        Err(error) => {
            println!("reverse: {error}");
            None
        }
    }
}

fn per_step(stats: &Stats) -> Option<f64> {
    let bytes = stats.journal_bytes?;
    (stats.steps > 0).then(|| bytes as f64 / stats.steps as f64)
}

/// A number of steps and the wall time they took, reported as throughput and never checked.
#[derive(Clone, Copy)]
struct Timed {
    steps: u64,
    time: Duration,
}

impl std::fmt::Display for Timed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let seconds = self.time.as_secs_f64();
        if seconds > 0.0 {
            write!(f, "{:.3e} steps/s ({} steps in {seconds:.3} s)", self.steps as f64 / seconds, self.steps)
        } else {
            write!(f, "{} steps, too fast to time", self.steps)
        }
    }
}

fn report_stats(stats: &Stats, forward: Timed, reverse: Option<Timed>) {
    let bound = |limit: Option<String>| limit.unwrap_or_else(|| "none".to_owned());
    println!("stats:");
    println!(
        "  bounds: step budget {}, basis configurations {}",
        bound(stats.budget.map(|b| b.to_string())),
        bound(stats.max_configurations.map(|m| m.to_string()))
    );
    println!("  forward: {forward}");
    if let Some(reverse) = reverse {
        println!("  reverse: {reverse}");
    }
    println!("  region steps: {}", stats.region_steps);
    match (stats.journal_bytes, per_step(stats)) {
        (Some(bytes), Some(per_step)) => println!("  journal: {bytes} bytes, {per_step:.1} bytes per step"),
        (Some(bytes), None) => println!("  journal: {bytes} bytes"),
        (None, _) => println!("  journal: off (run with --reverse to keep one)"),
    }
    println!("  qubits: {} live, {} peak", stats.live_qubits, stats.peak_live_qubits);
    println!("  basis configurations: {} now, {} peak", stats.configurations, stats.peak_configurations);
}
