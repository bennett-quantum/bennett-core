//! `bennett`: run and check device programs.

mod qasm;
mod run;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bennett_exec::StartError;
use bennett_ir::LoadError;
use clap::{Args, Parser, Subcommand, ValueEnum};
use thiserror::Error;

/// Run and check Bennett device programs.
#[derive(Parser)]
#[command(name = "bennett", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a program and report its result or fault, console output and step count.
    ///
    /// Exits with 0 when the program returns, 1 when it faults or (unless allowed) an apply faults
    /// for some argument value, and 2 when it cannot be run or stops at a device or simulator limit.
    Run(RunArgs),
    /// Read and validate a program. Exits with 0 when it is valid, 2 otherwise.
    Check {
        /// The program text.
        file: PathBuf,
    },
}

#[derive(Args)]
struct RunArgs {
    /// The program text.
    file: PathBuf,
    /// The quantum device.
    #[arg(long, value_enum, default_value_t = DeviceKind::Simulator)]
    device: DeviceKind,
    /// The new OpenQASM 3 file to write with --device qasm.
    #[arg(long, value_name = "FILE")]
    qasm_output: Option<PathBuf>,
    /// Where --device qasm takes measurement outcomes: the run's own simulation (the default), or synthetic
    /// bits for a circuit whose state the simulation cannot hold.
    #[arg(long, value_enum)]
    qasm_outcomes: Option<QasmOutcomes>,
    /// Compile applied functions to gates. Count and QASM default to auto, and the simulator defaults to evaluation.
    #[arg(long, value_enum)]
    apply_circuit: Option<ApplyCircuit>,
    /// The function to run.
    #[arg(long, default_value = "main")]
    entry: String,
    /// An argument of the entry function, in order; negative values are two's complement.
    #[arg(long = "arg", value_name = "N", allow_negative_numbers = true)]
    args: Vec<i128>,
    /// The most steps the run may take.
    #[arg(long, value_name = "N")]
    budget: Option<u64>,
    /// The most basis configurations the quantum state may hold; a gate beyond it stops the run. A limit of
    /// the simulator, not a fault of the program. It also bounds the simulation behind --device qasm.
    #[arg(long, value_name = "N")]
    max_configurations: Option<usize>,
    /// Let an `apply` fault for some values of its arguments and the run go on, for programs that inspect
    /// their fault registers on purpose. Without it the first such `apply` ends the run as a region fault:
    /// the program could not detect it with certainty, and the other values would go on with the
    /// interference gone.
    #[arg(long)]
    allow_region_faults: bool,
    /// Run the machine's meaning instead of the simulator's strict mode: a fault for some argument values of
    /// an `apply` is a fault code the program may measure. Since `qfree` became the partial trace this is the
    /// same as --allow-region-faults.
    #[arg(long)]
    machine_meaning: bool,
    /// The seed for simulator and QASM measurement sampling, or for synthetic count measurements.
    #[arg(long, value_name = "N", default_value_t = 0)]
    seed: u64,
    /// Run forward, then backward, and report whether the state returned to the entry state.
    #[arg(long)]
    reverse: bool,
    /// Report throughput, journal size, qubits and basis configurations.
    #[arg(long)]
    stats: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum DeviceKind {
    Simulator,
    Count,
    Qasm,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum QasmOutcomes {
    /// Sample each outcome from the run's own simulation.
    Simulated,
    /// Take seeded synthetic bits, as --device count does; an outcome can have probability zero.
    Synthetic,
}

#[derive(Clone, Copy, ValueEnum)]
enum ApplyCircuit {
    Auto,
    StepMachine,
}

/// Why a command could not do its work.
#[derive(Debug, Error)]
enum CliError {
    #[error("cannot read {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path}: {source}")]
    Load { path: String, source: LoadError },
    #[error("no function @{0}")]
    NoEntry(String),
    #[error("@{function} takes {expected} arguments, {found} given")]
    ArgumentCount { function: String, expected: usize, found: usize },
    #[error("argument {index} ({value}) does not fit in {ty}")]
    ArgumentRange { index: usize, value: i128, ty: bennett_ir::Type },
    #[error("--reverse requires the simulator")]
    DeviceReverse,
    #[error("--device qasm requires --qasm-output FILE")]
    QasmOutputRequired,
    #[error("--qasm-output requires --device qasm")]
    QasmDeviceRequired,
    #[error("--qasm-outcomes requires --device qasm")]
    QasmOutcomesDevice,
    #[error(
        "the simulation that gives the OpenQASM measurement outcomes would hold more than {limit} basis \
         configurations; no trace written. Rerun with --qasm-outcomes synthetic for synthetic outcomes, or \
         raise --max-configurations"
    )]
    QasmSimulationLimit { limit: usize },
    #[error("cannot write OpenQASM output {}: {source}", path.display())]
    QasmIo { path: PathBuf, source: std::io::Error },
    #[error("{0}")]
    Start(#[from] StartError),
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run(args) => run::run(&args),
        Command::Check { file } => check(&file),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::from(2)
    })
}

fn check(file: &Path) -> Result<ExitCode, CliError> {
    let module = load(file)?;
    println!("ok: {} functions", module.functions.len());
    Ok(ExitCode::SUCCESS)
}

fn load(file: &Path) -> Result<bennett_ir::ValidModule, CliError> {
    let path = file.display().to_string();
    let text = fs::read_to_string(file).map_err(|source| CliError::Io { path: path.clone(), source })?;
    bennett_ir::load(&text).map_err(|source| CliError::Load { path, source })
}
