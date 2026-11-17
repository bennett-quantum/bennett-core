//! User-facing help and execution use the remaining language.
use std::process::Command;

#[test]
fn help_and_ordinary_reverse_stats_do_not_advertise_removed_machinery() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mutation-boundaries.bn");
    for args in [
        vec!["--help"],
        vec!["run", "--help"],
        vec!["run", fixture, "--entry", "main", "--arg", "7", "--reverse", "--stats"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_bennett")).args(&args).output().expect("CLI");
        assert!(result.status.success(), "{result:?}");
        let text = format!("{}{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        for word in ["mark", "rewind", "world", "worlds", "deadline", "deadlines"] {
            assert!(
                !text.split(|c: char| !c.is_ascii_alphanumeric()).any(|token| token.eq_ignore_ascii_case(word)),
                "{args:?}: {text}"
            );
        }
    }
}
