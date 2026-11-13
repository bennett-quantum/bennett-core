//! The harness for `bennett-front`'s tests: C goes through `clang` and `bennett-front` to a device program,
//! which `bennett-exec` runs. Build products go to `target/front-tests`.
//!
//! `bennett-front` is found at `target/front/bennett-front` (as `front/test.sh` builds it) or at
//! `$BENNETT_FRONT`; `clang` on the path or at `$CLANG`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use bennett_exec::{Executor, Options, Outcome, Snapshot, Stats};
use bennett_ir::{FuncId, Inst, ValidModule};
use num_complex::Complex64;

/// Steps allowed for each run of an applied function. The longest coherent computation in the tests,
/// `collatz_steps` on 8-bit inputs (at most 127 iterations of about ten instructions), needs a few
/// thousand.
pub const REGION_BUDGET: u64 = 100_000;

/// Stack bytes above static data. The deepest recursion in the tests, `split` on 8-bit inputs, is eight
/// frames of a few slots each.
pub const STACK_BYTES: u64 = 65_536;

/// Steps allowed for a whole run: far above what any test program takes, so that a runaway program
/// faults instead of hanging.
pub const RUN_BUDGET: u64 = 10_000_000;

/// Basis configurations allowed: the largest state in the tests is 256 configurations.
pub const MAX_CONFIGURATIONS: usize = 1 << 12;

/// The workspace root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where test programs are written.
pub fn out_dir() -> PathBuf {
    static CREATE: Once = Once::new();
    let dir = root().join("target/front-tests");
    CREATE.call_once(|| std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("cannot create {dir:?}: {e}")));
    dir
}

/// The C sources of the tests.
pub fn c_dir() -> PathBuf {
    root().join("front/tests/c")
}

fn front() -> PathBuf {
    std::env::var_os("BENNETT_FRONT").map_or_else(|| root().join("target/front/bennett-front"), PathBuf::from)
}

fn clang() -> PathBuf {
    std::env::var_os("CLANG").map_or_else(|| PathBuf::from("clang"), PathBuf::from)
}

fn run_tool(command: &mut Command) -> Result<String, String> {
    let output = command.output().unwrap_or_else(|e| panic!("cannot run {command:?}: {e}"));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() { Ok(String::from_utf8_lossy(&output.stdout).into_owned()) } else { Err(stderr) }
}

/// How `clang` compiles C for the front end: unoptimised but optimisable, since `bennett-front` runs its
/// own pipeline, with line tables for diagnostics and C names for readable device programs.
pub const CLANG_FLAGS: &[&str] =
    &["-O0", "-Xclang", "-disable-O0-optnone", "-gline-tables-only", "-fno-discard-value-names"];

/// Compiles `source` with `clang_flags`, lowers it with `bennett-front` and `front_flags`, and returns the
/// device program text, or `bennett-front`'s error output. `name` names the build products.
pub fn lower_text(source: &Path, name: &str, clang_flags: &[&str], front_flags: &[&str]) -> Result<String, String> {
    lower_text_with_budget(source, name, clang_flags, front_flags, REGION_BUDGET)
}

/// [`lower_text`] with a region budget of `region_budget` steps.
pub fn lower_text_with_budget(
    source: &Path,
    name: &str,
    clang_flags: &[&str],
    front_flags: &[&str],
    region_budget: u64,
) -> Result<String, String> {
    let dir = out_dir();
    let bitcode = dir.join(format!("{name}.bc"));
    let program = dir.join(format!("{name}.bn"));
    let include = root().join("front/include");
    run_tool(
        Command::new(clang())
            .args(clang_flags)
            .arg("-I")
            .arg(&include)
            .args(["-emit-llvm", "-c"])
            .arg(source)
            .arg("-o")
            .arg(&bitcode),
    )
    .unwrap_or_else(|e| panic!("clang failed on {source:?}:\n{e}"));
    run_tool(
        Command::new(front())
            .arg(&bitcode)
            .arg("-o")
            .arg(&program)
            .arg(format!("--region-budget={region_budget}"))
            .arg(format!("--stack-bytes={STACK_BYTES}"))
            .args(front_flags),
    )?;
    Ok(std::fs::read_to_string(&program).unwrap_or_else(|e| panic!("cannot read {program:?}: {e}")))
}

/// Lowers `source` and loads the device program, which must be valid.
pub fn lower(source: &Path, name: &str, clang_flags: &[&str], front_flags: &[&str]) -> ValidModule {
    let text = lower_text(source, name, clang_flags, front_flags).unwrap_or_else(|e| panic!("{name}: {e}"));
    bennett_ir::load(&text).unwrap_or_else(|e| panic!("{name}: the device program is invalid: {e}\n{text}"))
}

/// Writes a C program into the build directory and returns its path.
pub fn write_c(name: &str, text: &str) -> PathBuf {
    let path = out_dir().join(format!("{name}.c"));
    std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
    path
}

/// The function named `name`.
pub fn entry(module: &ValidModule, name: &str) -> FuncId {
    module.function_named(name).unwrap_or_else(|| panic!("no function @{name}"))
}

/// Options for a run: bounded, with `seed`, and strict: an `apply` that faults on any branch stops the run
/// (`RunError::RegionFault`), since nothing a program from `bennett-front` does could detect it.
pub fn options(seed: u64) -> Options {
    Options {
        budget: Some(RUN_BUDGET),
        max_configurations: Some(MAX_CONFIGURATIONS),
        seed,
        reversible: false,
        allow_region_faults: false,
    }
}

/// Runs `name(args)` to the end.
pub fn run(module: &ValidModule, name: &str, args: &[u64], seed: u64) -> Outcome {
    run_with_stats(module, name, args, seed).0
}

/// Runs `name(args)` to the end and returns the outcome and the run's counts.
pub fn run_with_stats(module: &ValidModule, name: &str, args: &[u64], seed: u64) -> (Outcome, Stats) {
    let mut executor =
        Executor::new(module, entry(module, name), args, options(seed)).unwrap_or_else(|e| panic!("@{name}: {e}"));
    (executor.run(), executor.stats())
}

/// Runs `name(args)` up to, not including, its first measurement and returns the state there.
pub fn state_before_first_measurement(module: &ValidModule, name: &str, args: &[u64]) -> Snapshot {
    let mut executor =
        Executor::new(module, entry(module, name), args, options(0)).unwrap_or_else(|e| panic!("@{name}: {e}"));
    loop {
        let snapshot = executor.snapshot();
        let frame = snapshot.classical.frames.last().unwrap_or_else(|| panic!("@{name} has no frame"));
        let block = module.function(frame.func).block(frame.block);
        if let Some(Inst::Measure { .. }) = block.insts.get(frame.inst) {
            return snapshot;
        }
        if let Some(outcome) = executor.step() {
            panic!("@{name} ended ({outcome:?}) before measuring anything");
        }
    }
}

/// The names of the registers `name` allocates in its entry block, in order: the columns of the quantum
/// state while they are all live and nothing was allocated before.
pub fn registers(module: &ValidModule, name: &str) -> Vec<String> {
    let function = module.function(entry(module, name));
    function.blocks[0]
        .insts
        .iter()
        .filter_map(|inst| match inst {
            Inst::Qalloc { dst, .. } => Some(function.slot_names[dst.index()].clone()),
            _ => None,
        })
        .collect()
}

/// Whether register `name` is a fault register: `bennett-front` names each after the variable it guards,
/// `<variable>.fault`, and C names have no dots.
pub fn is_fault_register(name: &str) -> bool {
    name.contains(".fault")
}

/// The state of `name(args)` just before its first measurement with the fault registers left out, after
/// checking that they are 0 on every branch: two programs that differ only in how many `apply`s they
/// make have the same such state. Configurations are in order.
pub fn state_without_fault_registers(module: &ValidModule, name: &str, args: &[u64]) -> Vec<(Vec<u64>, Complex64)> {
    let columns = registers(module, name);
    let state = state_before_first_measurement(module, name, args).quantum;
    let mut kept: Vec<(Vec<u64>, Complex64)> = state
        .amplitudes
        .iter()
        .map(|(config, amplitude)| {
            let mut values = Vec::new();
            for (column, register) in columns.iter().enumerate() {
                if is_fault_register(register) {
                    assert_eq!(config[column], 0, "@{name}: fault register %{register} on {config:?}");
                } else {
                    values.push(config[column]);
                }
            }
            assert!(config[columns.len()..].iter().all(|&v| v == 0), "@{name}: registers beyond the entry's");
            (values, *amplitude)
        })
        .collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0));
    kept
}

/// The column of register `register` among `columns`.
pub fn column(columns: &[String], register: &str) -> usize {
    columns.iter().position(|c| c == register).unwrap_or_else(|| panic!("no register %{register} in {columns:?}"))
}

/// Compiles the C program `source` natively (`clang -O2` and `flags`) and returns the executable.
pub fn native_build(source: &Path, name: &str, flags: &[&str]) -> PathBuf {
    let executable = out_dir().join(format!("{name}.native"));
    run_tool(Command::new(clang()).arg("-O2").args(flags).arg(source).arg("-o").arg(&executable))
        .unwrap_or_else(|e| panic!("cannot compile {source:?} natively:\n{e}"));
    executable
}

/// Compiles the C program `source` natively, runs it, and returns its exit status.
pub fn native_exit_status(source: &Path, name: &str) -> i32 {
    let executable = native_build(source, name, &[]);
    let status = Command::new(&executable).status().unwrap_or_else(|e| panic!("cannot run {executable:?}: {e}"));
    status.code().unwrap_or_else(|| panic!("{executable:?} was killed: {status}"))
}

/// Compiles `native.c` (the corpus) natively, once per test process, and returns the executable.
pub fn native_corpus() -> PathBuf {
    static BUILD: Once = Once::new();
    let executable = out_dir().join("native-corpus");
    BUILD.call_once(|| {
        run_tool(Command::new(clang()).arg("-O2").arg(c_dir().join("native.c")).arg("-o").arg(&executable))
            .unwrap_or_else(|e| panic!("cannot compile the native corpus:\n{e}"));
    });
    executable
}

/// The native results of corpus function `name` on `inputs`, as unsigned 32-bit values.
pub fn native_results(name: &str, inputs: &[u64]) -> Vec<u64> {
    let mut child = Command::new(native_corpus())
        .arg(name)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot run the native corpus: {e}"));
    let text: String = inputs.iter().map(|x| format!("{x}\n")).collect();
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap_or_else(|| panic!("no stdin"));
        stdin.write_all(text.as_bytes()).unwrap_or_else(|e| panic!("cannot write inputs: {e}"));
    }
    let output = child.wait_with_output().unwrap_or_else(|e| panic!("native corpus: {e}"));
    assert!(output.status.success(), "native {name} failed");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.parse().unwrap_or_else(|e| panic!("native {name}: {line:?}: {e}")))
        .collect()
}

/// The corpus functions, as `corpus.c` lists them.
pub fn corpus_names() -> Vec<String> {
    let text = std::fs::read_to_string(c_dir().join("corpus.c")).unwrap_or_else(|e| panic!("corpus.c: {e}"));
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("X(").and_then(|rest| rest.split(')').next()))
        .map(str::to_owned)
        .collect()
}
