//! Raw-sweep acceptance, independently constructed bit patterns. Requirement:
//! HANDOFF's afternoon NaN decision and rt/libgcc/README.md's exact rule.

use std::path::Path;
use std::process::Command;

fn probe(code: &str) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rt/libgcc/exact/compare.py");
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-c", code])
        .arg(script)
        .output()
        .expect("run NaN rule probe");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// For each format choose a payload in the operands, its sign-flipped result,
/// the default NaN, an unrelated payload and a finite value. Only the first two
/// NaN payloads are permitted. Fields are explicitly laid out, without asking
/// compare.py to generate operands or calculate an expected result.
#[test]
fn independently_constructed_nan_payloads_accept_only_the_allowed_set() {
    probe(
        r#"
import runpy, sys
agree = runpy.run_path(sys.argv[1])['agrees']
formats = [(8, 0x7f800000, 1 << 22, 1 << 31),
           (16, 0x7ff0000000000000, 1 << 51, 1 << 63),
           (20, (0x7fff << 64) | (1 << 63), 1 << 62, 1 << 79)]
for width, exponent, quiet, sign in formats:
    default = exponent | quiet
    for bits in [1, 3, 17, 257, 65537]:
        chosen = default | bits
        unknown = default | (bits << 1)
        operands = (chosen, 0, 0, 0)
        assert agree(chosen, default, width, operands)
        assert agree(chosen ^ sign, chosen, width, operands)
        assert not agree(unknown, default, width, operands)
        assert not agree(chosen, 0, width, operands)
        assert not agree(0, sign, width, operands)  # zero signs still matter
"#,
    );
}

/// The rule requires each differing output NaN to have its quiet bit set. A
/// signalling output is not merely a different permitted payload. This witness
/// deliberately differs in that bit only, so masking/quieting the outputs is wrong.
#[test]
fn a_signalling_output_is_not_accepted_as_another_quiet_nan_payload() {
    probe(
        r#"
import runpy, sys
agree = runpy.run_path(sys.argv[1])['agrees']
for width, signal, quiet in [
    (8, 0x7f800001, 0x7fc00001),
    (16, 0x7ff0000000000001, 0x7ff8000000000001),
    (20, (0x7fff << 64) | 0x8000000000000001, (0x7fff << 64) | 0xc000000000000001),
]:
    assert not agree(signal, quiet, width, (quiet, 0, 0, 0)), (width, 'signalling output accepted')
"#,
    );
}
