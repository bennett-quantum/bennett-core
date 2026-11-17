//! `printf("%a")` with musl linked, against the native reference (clang 18 with the host's glibc). Normal
//! values agree. Subnormals do not: musl normalises them (`frexpl`, `0x1p-1074`) where glibc prints the
//! denormal form (`0x0.0000000000001p-1022`); C leaves the first digit of a subnormal unspecified, so both
//! libraries conform and the values are equal. A native musl 1.2.5 build prints exactly what the device
//! prints, so the device runs musl faithfully: the difference is the library's. Found by the Lua probe's
//! `format_corners` script (`experiments/lua-probe/README.md`). Whether the device should follow glibc
//! here, as it does for a copied `sigjmp_buf`, is a question for the maintainer.
use bennett_exec::{Executor, Options, Outcome};
use bennett_front_tests::{native_build, out_dir, root, write_c};
use std::process::Command;

/// A bound on the steps of one run, far above the program's.
const STEP_BUDGET: u64 = 100_000_000;

fn device_and_native(name: &str, source: &str) -> (Vec<u8>, Vec<u8>) {
    let source = write_c(name, source);
    let native = Command::new(native_build(&source, name, &[])).output().expect("native");
    assert_eq!(native.status.code(), Some(0));
    let workspace = root().canonicalize().expect("workspace");
    let build = Command::new("python3").arg(workspace.join("rt/musl/build.py")).output().expect("musl build");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let file = out_dir().join(format!("{name}.bn"));
    let link = Command::new("python3")
        .arg(workspace.join("rt/musl/link.py"))
        .arg(&source)
        .arg("-o")
        .arg(&file)
        .output()
        .expect("musl link");
    assert!(link.status.success(), "{}", String::from_utf8_lossy(&link.stderr));
    let module = bennett_ir::load(&std::fs::read_to_string(&file).expect("device text")).expect("valid");
    let entry = module.function_named("__bennett_entry").expect("entry");
    let options = Options { budget: Some(STEP_BUDGET), ..Options::default() };
    let mut e = Executor::new(&module, entry, &[], options).expect("start");
    assert_eq!(e.run(), Outcome::Returned(Some(0)));
    (e.tape().to_vec(), native.stdout)
}

#[test]
fn hexadecimal_floats_of_normal_values_agree_with_native() {
    let (device, native) = device_and_native(
        "printf_hex_normal",
        "#include <stdio.h>\nint main(void) {\n  printf(\"%a %a %a %A %.3a %a %a\\n\", 1.0, 0.1, -2.5, 255.5, 1.0 / 3, 0x1p-1022, -0.0);\n  return 0;\n}\n",
    );
    assert_eq!(String::from_utf8_lossy(&device), String::from_utf8_lossy(&native));
}

#[test]
#[ignore = "defect: %a of a subnormal double is musl's normalised spelling on the device, glibc's denormal one natively"]
fn hexadecimal_floats_of_subnormals_agree_with_native() {
    let (device, native) = device_and_native(
        "printf_hex_subnormal",
        "#include <stdio.h>\nint main(void) {\n  printf(\"%a %a %.3a\\n\", 0x1p-1074, 0x1.8p-1030, 0x1.fffffp-1023);\n  return 0;\n}\n",
    );
    // glibc: 0x0.0000000000001p-1022 0x0.018p-1022 0x1.000p-1022; musl: 0x1p-1074 0x1.8p-1030 0x2.000p-1023.
    assert_eq!(String::from_utf8_lossy(&device), String::from_utf8_lossy(&native));
}
