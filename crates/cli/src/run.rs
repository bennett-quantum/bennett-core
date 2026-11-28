//! `bennett run`: execute, optionally reverse, and report.

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use bennett_emit::{ApplyMode, CountingDevice, LowerControls, compile_apply_with, seeded_bits};
use bennett_exec::{
    ApplyCompiler, ApplyError, ApplyInput, ApplyReport, Executor, Options, Outcome, ReverseError, RunError,
    SimulatedDevice, Snapshot, Stats,
};
use bennett_ir::{Const, FuncId, Type, ValidModule};

use crate::qasm::{Outcomes, QasmDevice, write_run_end};
use crate::{ApplyCircuit, CliError, DeviceKind, QasmOutcomes, RunArgs};

/// The most basis configurations the simulation behind `--device qasm` holds unless `--max-configurations`
/// says otherwise: 2^20, tens of megabytes of amplitudes. A circuit beyond it is refused by name, naming
/// `--qasm-outcomes synthetic`.
const QASM_SIMULATION_CONFIGURATIONS: usize = 1 << 20;

pub(crate) fn run(args: &RunArgs) -> Result<ExitCode, CliError> {
    let module = crate::load(&args.file)?;
    let entry = module.function_named(&args.entry).ok_or_else(|| CliError::NoEntry(args.entry.clone()))?;
    let values = entry_arguments(&module, entry, &args.args)?;
    if args.reverse && !matches!(args.device, DeviceKind::Simulator) {
        return Err(CliError::DeviceReverse);
    }
    if args.qasm_outcomes.is_some() && !matches!(args.device, DeviceKind::Qasm) {
        return Err(CliError::QasmOutcomesDevice);
    }
    match (args.device, args.qasm_output.as_deref()) {
        (DeviceKind::Simulator, None) => run_simulator(&module, entry, &values, args),
        (DeviceKind::Count, None) => run_count(&module, entry, &values, args),
        (DeviceKind::Qasm, Some(path)) => run_qasm(&module, entry, &values, args, path),
        (DeviceKind::Qasm, None) => Err(CliError::QasmOutputRequired),
        (DeviceKind::Simulator | DeviceKind::Count, Some(_)) => Err(CliError::QasmDeviceRequired),
    }
}

fn run_count(module: &ValidModule, entry: FuncId, values: &[u64], args: &RunArgs) -> Result<ExitCode, CliError> {
    let mut device = LowerControls::new(CountingDevice::new(seeded_bits(args.seed)));
    let mut compiler = gate_compiler(args.apply_circuit.unwrap_or(ApplyCircuit::Auto), print_report);
    let run = Executor::run_on_with_apply(module, entry, values, args.budget, &mut device, &mut compiler)?;
    report_result(module, entry, run.outcome, 0.0);
    report_io(&run.tape, run.steps);
    let counts = device.inner().counts();
    println!("measurement source: synthetic bits (seed {})", args.seed);
    let outcomes: String = device.inner().outcomes().iter().map(|&bit| if bit { '1' } else { '0' }).collect();
    println!("measurement outcomes: {}", if outcomes.is_empty() { "none" } else { &outcomes });
    println!("one-qubit gates: {}", counts.one_qubit);
    println!("two-qubit gates: {}", counts.two_qubit);
    println!("Toffoli gates: {}", counts.toffoli);
    println!("peak qubits: {}", counts.peak_qubits);
    println!("depth: {}", counts.depth);
    Ok(ExitCode::from(outcome_status(run.outcome)))
}

fn run_qasm(
    module: &ValidModule,
    entry: FuncId,
    values: &[u64],
    args: &RunArgs,
    path: &Path,
) -> Result<ExitCode, CliError> {
    let io_error = |source| CliError::QasmIo { path: path.to_owned(), source };
    // Reject an existing path before running, and check again when publishing.
    match path.symlink_metadata() {
        Ok(_) => return Err(io_error(io::ErrorKind::AlreadyExists.into())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let outcomes = match args.qasm_outcomes.unwrap_or(QasmOutcomes::Simulated) {
        QasmOutcomes::Simulated => {
            let limit = args.max_configurations.unwrap_or(QASM_SIMULATION_CONFIGURATIONS);
            let simulation = SimulatedDevice::new(args.seed, Some(limit));
            // A gate is checked against the limit as it acts; the initial state, before any, is checked here.
            if simulation.configurations() > limit {
                return Err(CliError::QasmSimulationLimit { limit });
            }
            Outcomes::Simulated(Box::new(simulation))
        }
        QasmOutcomes::Synthetic => Outcomes::Synthetic(Box::new(seeded_bits(args.seed))),
    };
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    // Use regular file creation so the staged file has the intended permissions when published.
    let staged = tempfile::Builder::new().make_in(directory, |path| File::create_new(path)).map_err(io_error)?;
    let mut output = BufWriter::new(staged);
    // The compiler's report of each `apply` is printed only once the trace is published.
    let mut diagnostics = Vec::new();
    let run = {
        let writer = QasmDevice::new(&mut output, args.seed, outcomes).map_err(io_error)?;
        let mut device = LowerControls::new(writer);
        let mut compiler =
            gate_compiler(args.apply_circuit.unwrap_or(ApplyCircuit::Auto), |line| diagnostics.push(line));
        let run = Executor::run_on_with_apply(module, entry, values, args.budget, &mut device, &mut compiler);
        if let Some(source) = device.inner().take_write_error() {
            return Err(io_error(source));
        }
        run?
    };
    // Only the simulation stops at the configuration limit: refuse rather than publish a partial trace.
    if let Outcome::Stopped(RunError::ConfigurationLimit { limit }) = run.outcome {
        return Err(CliError::QasmSimulationLimit { limit });
    }
    let status = outcome_status(run.outcome);
    write_run_end(&mut output, run.outcome, status).map_err(io_error)?;
    let staged = output.into_inner().map_err(|error| io_error(error.into_error()))?;
    staged.persist_noclobber(path).map_err(|error| io_error(error.error))?;
    diagnostics.into_iter().for_each(print_report);
    report_result(module, entry, run.outcome, 0.0);
    report_io(&run.tape, run.steps);
    match args.qasm_outcomes.unwrap_or(QasmOutcomes::Simulated) {
        QasmOutcomes::Simulated => println!("measurement source: simulation (seed {})", args.seed),
        QasmOutcomes::Synthetic => println!("measurement source: synthetic bits (seed {})", args.seed),
    }
    Ok(ExitCode::from(status))
}

/// The gate compiler, giving `report` one line for each `apply` it compiles, as it compiles it.
fn gate_compiler(choice: ApplyCircuit, mut report: impl FnMut(String)) -> impl ApplyCompiler {
    let mode = match choice {
        ApplyCircuit::Auto => ApplyMode::Automatic,
        ApplyCircuit::StepMachine => ApplyMode::StepMachine,
    };
    move |input: ApplyInput<'_>| {
        let function = &input.module.function(input.callee).name;
        let compiled = compile_apply_with(input, mode)?;
        report(format!("apply @{function} uses {} ({})", compiled.construction, compiled.reason));
        report(format!("apply @{function} memory: {}", compiled.memory));
        Ok(compiled.block)
    }
}

fn print_report(line: String) {
    println!("{line}");
}

fn run_simulator(module: &ValidModule, entry: FuncId, values: &[u64], args: &RunArgs) -> Result<ExitCode, CliError> {
    let options = Options {
        budget: args.budget,
        max_configurations: args.max_configurations,
        seed: args.seed,
        reversible: args.reverse,
        allow_region_faults: args.allow_region_faults,
        machine_meaning: args.machine_meaning,
    };
    let mut executor = Executor::new(module, entry, values, options)?;
    if args.stats {
        executor.keep_apply_reports();
    }
    let entry_state = executor.snapshot();

    let started = Instant::now();
    let outcome = match args.apply_circuit {
        Some(choice) => executor.run_emitted(&mut gate_compiler(choice, print_report)),
        None => executor.run(),
    };
    let forward = Timed { steps: executor.stats().steps, time: started.elapsed() };
    let stats = executor.stats();
    report_result(module, entry, outcome, stats.region_fault_max_weight);
    report_simulator_diagnostics(outcome, &stats);
    report_io(executor.tape(), stats.steps);

    let reverse = args.reverse.then(|| reverse_and_report(&mut executor, &entry_state, &stats));
    if args.stats {
        report_stats(&stats, forward, reverse.flatten());
        report_applies(module, executor.apply_reports());
    }
    Ok(ExitCode::from(outcome_status(outcome)))
}

/// One line per `apply` the simulator evaluated directly: the steps its inputs used, from which a program
/// takes its budget (`docs/DESIGN.md`, "Step budgets").
fn report_applies(module: &ValidModule, reports: &[ApplyReport]) {
    for report in reports {
        let largest = report.largest.map_or_else(|| "none".to_owned(), |steps| steps.to_string());
        println!(
            "  apply {} @{} budget {}: {} inputs, largest step count {largest}, {} at the budget",
            report.position,
            module.function(report.callee).name,
            report.budget,
            report.inputs,
            report.at_budget
        );
    }
}

fn outcome_status(outcome: Outcome) -> u8 {
    match outcome {
        Outcome::Returned(_) => 0,
        Outcome::Faulted(_) | Outcome::Stopped(RunError::RegionFault { .. }) => 1,
        Outcome::Stopped(_) => 2,
    }
}

/// The entry arguments as canonical values of the parameter widths.
fn entry_arguments(module: &ValidModule, entry: FuncId, literals: &[i128]) -> Result<Vec<u64>, CliError> {
    let function = module.function(entry);
    if function.params.len() != literals.len() {
        let (expected, found) = (function.params.len(), literals.len());
        return Err(CliError::ArgumentCount { function: function.name.clone(), expected, found });
    }
    let convert = |(index, (param, &value)): (usize, (&bennett_ir::Param, &i128))| match param.ty {
        Type::Int(width) => Const::from_literal(width, value < 0, value.unsigned_abs())
            .map(Const::value)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(CliError::ArgumentRange { index, value, ty: param.ty }),
        // The executor rejects register parameters with its own error.
        Type::Qreg(_) => Ok(0),
    };
    function.params.iter().zip(literals).enumerate().map(convert).collect()
}

fn report_result(module: &ValidModule, entry: FuncId, outcome: Outcome, region_fault_weight: f64) {
    match outcome {
        Outcome::Returned(None) => match module.function(entry).result {
            Some(Type::Qreg(qubits)) => println!("result: register (q{})", qubits.count()),
            Some(Type::Int(_)) | None => println!("result: none"),
        },
        Outcome::Returned(Some(value)) => match module.function(entry).result {
            Some(Type::Int(width)) if width.bits() > 1 && value >> (width.bits() - 1) == 1 => {
                let signed = (value | !width.mask()).cast_signed();
                println!("result: {value} (signed {signed})");
            }
            _ => println!("result: {value}"),
        },
        Outcome::Faulted(fault) => println!("fault: {fault} (code {})", fault.code()),
        Outcome::Stopped(RunError::RegionFault { callee, fault, faulting, configurations }) => println!(
            "region fault: apply @{} faulted ({fault}) on {faulting} of {configurations} basis configurations, weight {:e}; \
             the program could not detect this with certainty (--allow-region-faults lets the run go on)",
            module.function(callee).name,
            region_fault_weight
        ),
        Outcome::Stopped(RunError::Apply(ApplyError::Unsupported { callee, block, index, operation, route })) => {
            let function = module.function(callee);
            println!(
                "stopped: cannot emit {operation} in @{} at {} instruction {index}: needs {route}",
                function.name,
                function.block(block).name,
            );
        }
        Outcome::Stopped(error) => println!("stopped: {error}"),
    }
}

fn report_simulator_diagnostics(outcome: Outcome, stats: &Stats) {
    if !matches!(outcome, Outcome::Stopped(RunError::RegionFault { .. })) && stats.region_faults > 0 {
        println!(
            "region faults (allowed): {} apply executions faulted for some argument value, total weight {:e}",
            stats.region_faults, stats.region_fault_weight
        );
    }
    if stats.nonzero_releases > 0 {
        println!(
            "simulator report: {} `qfree` released a register that was not zero, weight {:e} in all; the release \
             is the partial trace and the program cannot see this ({} qubits kept as unnamed columns)",
            stats.nonzero_releases, stats.nonzero_release_weight, stats.leftover_qubits
        );
    }
}

fn report_io(tape: &[u8], steps: u64) {
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
    println!(
        "  region faults: {} apply executions faulted for some argument value; weight {:e} in all, {:e} at most",
        stats.region_faults, stats.region_fault_weight, stats.region_fault_max_weight
    );
    match (stats.journal_bytes, per_step(stats)) {
        (Some(bytes), Some(per_step)) => println!("  journal: {bytes} bytes, {per_step:.1} bytes per step"),
        (Some(bytes), None) => println!("  journal: {bytes} bytes"),
        (None, _) => println!("  journal: off (run with --reverse to keep one)"),
    }
    println!(
        "  qubits: {} live, {} peak, {} left over; {} memory bytes hold qubits",
        stats.live_qubits, stats.peak_live_qubits, stats.leftover_qubits, stats.memory_qubit_bytes
    );
    println!("  basis configurations: {} now, {} peak", stats.configurations, stats.peak_configurations);
}
