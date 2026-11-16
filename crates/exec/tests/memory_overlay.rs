//! Memory as an overlay of writes over a classical base (`docs/DESIGN.md`, "Memory as an overlay"): a
//! split copies what the world wrote, never the memory, so what the worlds hold grows with the bytes they
//! wrote and not with the memory's size; worlds whose memories are equal again merge, whatever order
//! their writes came in.

mod common;

use bennett_exec::{Executor, Options, Outcome, Stats};
use common::{TOLERANCE, load};
use num_complex::Complex64;

/// Eight worlds that each write `n` bytes: `q` (2 qubits) is read, each of its four worlds writes `n`
/// bytes of `q + 1` from address 64 up (even `q`) or down (odd `q`); then `r` (1 qubit) is read, which
/// splits each of them again, copying its overlay. The rewind un-runs both reads and the writes, so the
/// eight worlds merge and the Hadamards undo the preparation: the state is |0> again.
fn program(memory: u64) -> String {
    format!(
        "memory {memory}

func @write(i2 %x, i64 %n) -> i2 {{
entry:
    %v = zext i2 %x to i8
    %b = add i8 %v, 1
    %odd = trunc i2 %x to i1
    br loop(i64 0)
loop(i64 %i):
    %more = icmp ult i64 %i, %n
    br %more, body, done
body:
    %up = add i64 %i, 64
    %left = sub i64 %n, %i
    %down = add i64 %left, 63
    %a = select i64 %odd, %down, %up
    store i8 %b, %a
    %j = add i64 %i, 1
    br loop(%j)
done:
    ret %x
}}

func @id(i1 %x) -> i1 {{
entry:
    ret %x
}}

func @main(i64 %n) {{
entry:
    %q = qalloc 2
    h %q[0]
    h %q[1]
    %r = qalloc 1
    h %r
    %m = mark budget -1
    %a = call @write(%q, %n)
    %b = call @id(%r)
    rewind %m
    h %r
    h %q[1]
    h %q[0]
    ret
}}
"
    )
}

fn run(memory: u64, bytes: u64, reversible: bool) -> (Stats, Executor<'static>) {
    let module: &'static _ = Box::leak(Box::new(load(&program(memory))));
    let entry = module.function_named("main").expect("@main");
    let options = Options { budget: Some(1 << 20), reversible, ..Options::default() };
    let mut executor = Executor::new(module, entry, &[bytes], options).expect("starts");
    assert_eq!(executor.run(), Outcome::Returned(None), "memory {memory}, {bytes} bytes");
    (executor.stats(), executor)
}

/// The exact costs of a split, at three memory sizes (16 KiB, 1 MiB as the arena of a musl-linked
/// program, 4 MiB) and four numbers of bytes written: the bytes splits copy and the most the overlays
/// hold are functions of the bytes written alone, and memory is back to its start once the worlds merge.
#[test]
fn a_split_copies_the_bytes_written_not_the_memory() {
    for memory in [16 << 10, 1 << 20, 4 << 20] {
        for bytes in [0, 1, 64, 4096] {
            let (stats, executor) = run(memory, bytes, false);
            let context = format!("memory {memory}, {bytes} bytes: {stats:?}");
            assert_eq!((stats.splits, stats.merges, stats.peak_worlds, stats.worlds), (7, 7, 8, 1), "{context}");
            // Three worlds split off at the first read with nothing written; four at the second, each
            // copying the `bytes` its world wrote.
            assert_eq!(stats.split_memory_bytes, 4 * bytes, "{context}");
            assert_eq!(stats.peak_overlay_bytes as u64, 8 * bytes, "{context}");
            assert_eq!(stats.overlay_bytes, 0, "{context}: merged, so the one world holds no overlay");
            let snapshot = executor.snapshot();
            assert!(snapshot.classical.memory.iter().all(|&byte| byte == 0), "{context}: memory not restored");
            let one = snapshot.quantum.amplitude(&[0, 0]);
            assert!((one - Complex64::new(1.0, 0.0)).norm() <= TOLERANCE, "{context}: {one}");
        }
    }
}

/// The same run reversed returns to the start exactly, and the journal a split copies grows with the
/// steps run, not with the memory.
#[test]
fn the_runs_reverse_and_the_journal_copied_is_independent_of_memory() {
    let mut journal = Vec::new();
    for memory in [16 << 10, 1 << 20] {
        let (_, mut executor) = run(memory, 64, true);
        journal.push(executor.stats().split_journal_bytes);
        let module = load(&program(memory));
        let entry = module.function_named("main").expect("@main");
        let at_entry =
            Executor::new(&module, entry, &[64], Options { reversible: true, ..Options::default() }).expect("starts");
        executor.reverse().expect("reverses");
        let back = executor.snapshot();
        assert_eq!(back.classical, at_entry.snapshot().classical, "memory {memory}");
        assert!(back.quantum.distance(&at_entry.snapshot().quantum).is_some_and(|d| d <= TOLERANCE));
    }
    assert_eq!(journal[0], journal[1], "journal bytes copied by splits");
    assert!(journal[0] > 0);
}

/// Worlds that never merge: each writes its bytes and the run ends with four worlds, so the host observes
/// one. That world is left alone and owns the base: its overlay is folded in, and its memory holds exactly
/// what it wrote.
#[test]
fn a_world_left_alone_keeps_what_it_wrote_and_folds_it_into_the_base() {
    let text = program(1 << 12).replace("    %b = call @id(%r)\n    rewind %m\n", "");
    let module = load(&text);
    let entry = module.function_named("main").expect("@main");
    for seed in 0..8 {
        let options = Options { budget: Some(1 << 16), seed, ..Options::default() };
        let mut executor = Executor::new(&module, entry, &[16], options).expect("starts");
        assert_eq!(executor.run(), Outcome::Returned(None));
        let stats = executor.stats();
        assert_eq!((stats.worlds, stats.observations, stats.peak_overlay_bytes), (1, 1, 64), "{stats:?}");
        assert_eq!(stats.overlay_bytes, 0, "the world left alone folded its overlay");
        let snapshot = executor.snapshot();
        let value = snapshot.classical.memory[64];
        assert!((1..=4).contains(&value));
        assert!(snapshot.classical.memory[64..80].iter().all(|&byte| byte == value), "seed {seed}");
        assert!(snapshot.classical.memory.iter().filter(|&&byte| byte != 0).count() == 16, "seed {seed}");
    }
}

/// Host cost of one run, for the record (not a check): `BENNETT_MEMORY` and `BENNETT_BYTES` choose the
/// memory size and the bytes each world writes. Run alone, in a release build, one configuration per
/// process, so that the peak resident size is this run's:
/// `BENNETT_MEMORY=1048576 BENNETT_BYTES=64 cargo test --release --test memory_overlay report -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement for the record"]
fn report() {
    let number = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let (memory, bytes) = (number("BENNETT_MEMORY", 1 << 20), number("BENNETT_BYTES", 64));
    let start = std::time::Instant::now();
    let (stats, _) = run(memory, bytes, false);
    let elapsed = start.elapsed();
    let peak_kib = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| status.lines().find(|line| line.starts_with("VmHWM:")).map(str::to_owned))
        .unwrap_or_default();
    println!(
        "memory {memory} bytes {bytes}: {:.3} ms; steps {} world steps {} splits {} merges {} peak worlds {}; {peak_kib}",
        elapsed.as_secs_f64() * 1e3,
        stats.steps,
        stats.world_steps,
        stats.splits,
        stats.merges,
        stats.peak_worlds
    );
    println!(
        "  split memory bytes {} split journal bytes {} peak overlay bytes {}",
        stats.split_memory_bytes, stats.split_journal_bytes, stats.peak_overlay_bytes
    );
}
