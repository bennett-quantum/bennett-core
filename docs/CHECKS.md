# Automatic checks

Run `scripts/check.sh` for the four repository checks and the tree guards. Rust tests also run the
small guard self-tests (Python 3 required); LLVM is needed only for `front/test.sh`. All checks use
source discovery, compiler exhaustiveness, or exact outcomes: no coverage percentage, timing limit,
allowed-hit baseline, or list of example names is maintained.

| Check | Entry point and rejection witness |
|---|---|
| Changed-line mutation | `python3 scripts/mutate-diff.py origin/main`, **after committing the step and before landing**. A surviving replacement makes cargo-mutants fail. `--list` previews scope and is not a passing mutation run. |
| Toolchain pin | `front/test.sh`, CMake configuration, and `front/tests/tests/toolchain.rs`. The actual clang, linked front LLVM, and runtime bitcode linker must have the major in `front/llvm-major.txt` (18). A wrong compiler on PATH or through CLANG is rejected. Patch releases are deliberately compatible with the existing LLVM 18 pin. |
| Validator admission | `cargo test -p bennett-ir --test admission`: accepted boundary programs round-trip; rejected programs require the exact validation problem, both reachable and in unused functions. Reader error cases remain in `text.rs`. |
| Example discovery | `cargo test -p bennett-ir --test text`: recursively discovers `examples/**/*.bn`; an added malformed example fails without changing a list. Executor tests separately check the examples' outcomes. |
| Attribute classification | `front/test.sh` compiles and runs `front/tests/checks.cpp`. Its exhaustive switch includes `front/src/Attributes.def`; deleting a policy, or adding an LLVM enum kind, fails compilation. Reasons distinguish explicit lowering, LLVM promises/hints with no device encoding, and named refusals. |
| Exact pushed revision | `.githooks/pre-push` archives each nondeleted SHA from Git's pre-push input, then runs `scripts/check.sh` in that isolated tree. A bad non-HEAD branch fails even when HEAD passes; dirty files and existing front binaries cannot repair it. |
| Tree hygiene | `python3 scripts/checks.py`: tracked and nonignored untracked source is scanned for host-specific home paths, nonlocal symlinks and generated/session litter. The archive hook scans the exact committed manifest. |
| No WIP on main | The same hook checks **all incoming commits**, including ancestors and a newly created main. WIP/work-in-progress/fixup/squash subjects fail on `refs/heads/main`; topic and `wip/` refs may contain them. |
| Exhaustive library matches | The IR and executor libraries deny `clippy::wildcard_enum_match_arm` outside their unit tests. `cargo clippy --all-targets -- -D warnings` enforces this; `scripts/check.sh` also runs clippy over **all workspace libraries** with this lint, discovering new library targets automatically. Test assertions may use catch-all arms. |
| Bare script durations | `python3 scripts/checks.py`: Python sleep/timeout literals and shell timeout/sleep literals are flagged; JavaScript timer literals are also flagged. Name durations with their unit and purpose, e.g. `NATIVE_TIMEOUT_SECONDS`. |

The hook witnesses and tree-guard witnesses are executable in `python3 scripts/test_checks.py`, also
called by `cargo test`. They create disposable repositories under `.tmp`, never push anything, and
exercise bad non-HEAD refs, a dirty checkout, branch deletion, and a WIP ancestor below a finished tip.

## Install the push hook

From this checkout:

```sh
git config --local core.hooksPath .githooks
```

This selects the versioned hook for this repository (including its worktrees). It replaces any existing
hooksPath setting, so combine hooks if another policy is already installed. The hook does not install
itself. A remote old SHA that is not available locally fails closed; fetch that remote before retrying.
It checks every nondeleted pushed revision, including checkpoint branches. No log from an earlier run
is accepted as evidence. Git's `--no-verify` can bypass local hooks; server-side enforcement is separate.

On a shared host, export `CARGO_BUILD_JOBS=2` and set `BENNETT_BUILD_WRAPPER` to a local executable that
accepts `build <command ...>`. `scripts/check.sh` routes builds/tests through it. The hook preserves that
setting. The default `front/test.sh` also caps CMake jobs using `CARGO_BUILD_JOBS` (default 2).

## Mutation testing is a separate landing step

Install the established tool with `cargo install cargo-mutants --locked`. Commit a step, then run:

```sh
python3 scripts/mutate-diff.py origin/main --list
python3 scripts/mutate-diff.py origin/main
```

The script checks a clean revision, uses its merge-base diff, rejects an empty mutation set, runs one mutant at a time, and tests all
workspace default packages rather than just the mutated crate. Results and the input diff go under
`.tmp/mutation/`. Fix every real surviving mutant with a regression test; examine unviable mutants and
timeouts rather than relabeling them as caught. Do not lower a score threshold or exclude a survivor to
land. For unsupported forms, seed a semantic edit by hand and run the relevant existing suite.

Scope and cost are explicit: cargo-mutants generates Rust mutations, not C++, Python, or shell
mutations. Front/script edits therefore require recorded seeded edits and their actual failing check;
the script's zero exit alone does not certify them. A front harness change also needs its LLVM-dependent
suite. Automatic changed-line mutation for those languages remains a debt (see HANDOFF). Enumeration
and an individual small witness take seconds; a full changed-line run scales with the number of
mutants and rebuild cost. No honest full mutation run has a size-independent seconds guarantee. Keep
it out of `cargo test`; record its revision, command, caught/missed/unviable/timeout counts and elapsed
work before landing. Cold builds and the isolated pre-push front build likewise take longer than warm
checks. These are resource costs, not semantic failures or wall-time gates.

The attribute policy covers every LLVM **enum** kind, discovered by the C++ compiler. LLVM's open-ended
string attributes cannot have a closed exhaustive list. Most enum promises are consumed by LLVM's
optimiser; classification is not a proof of support for every placement or custom calling convention.
The reasons record conditional handling and current debts. Native differential tests remain necessary.
