//! `bennett`: run and check device programs.

mod run;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bennett_exec::StartError;
use bennett_ir::LoadError;
use clap::{Args, Parser, Subcommand};
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
    /// Exits with 0 when the program returns, 1 when it faults, 2 when it cannot be run or the simulator
    /// stops it at one of its limits.
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
    /// the simulator, not a fault of the program.
    #[arg(long, value_name = "N")]
    max_configurations: Option<usize>,
    /// The seed for `measure`.
    #[arg(long, value_name = "N", default_value_t = 0)]
    seed: u64,
    /// Run forward, then backward, and report whether the state returned to the entry state.
    #[arg(long)]
    reverse: bool,
    /// Report throughput, journal size, qubits and basis configurations.
    #[arg(long)]
    stats: bool,
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
