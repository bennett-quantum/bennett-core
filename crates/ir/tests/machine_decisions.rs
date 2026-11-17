//! Admission and text probes independent of the implementation's dataflow.
use bennett_ir::{load, read, write};

#[test]
fn all_measurement_widths_and_extreme_addresses_round_trip() {
    for width in 1..=128 {
        for address in ["0", "17", "0xffffffffffffffff", "-1", "%p"] {
            let text = format!(
                "memory 512\nfunc @main(i64 %p) -> i{width} {{\nentry:\n %m = measure i{width} [{address}]\n ret %m\n}}"
            );
            let module = read(&text).expect("read");
            assert_eq!(read(&write(&module)), Ok(module));
            load(&text).unwrap_or_else(|e| panic!("{text}\n{e}"));
        }
    }
}
