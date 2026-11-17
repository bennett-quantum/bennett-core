use bennett_exec::{Options, Outcome, ReverseError};
use bennett_ir::Fault;
use num_complex::Complex64 as C;

use crate::common::{bounded, executor, load};

/// DESIGN, "Qubits in memory": exactly N little-endian bits, with partial cells left coherent.
/// Put one H-prepared bit inside and an S H-prepared bit immediately outside the measurement.
/// Explicitly check the two surviving complex amplitudes, including when both bits share a cell.
#[test]
fn every_integer_width_measures_only_its_bits_at_unaligned_addresses() {
    let mut runs = 0;
    for width in 1u32..=128 {
        for address in [17u64, 31] {
            let outside_address = address + u64::from(width / 8);
            let outside_bit = width % 8;
            let mut bytes: Vec<u8> = (0..17).map(|i| (i as u8).wrapping_mul(37).wrapping_add(0xA4)).collect();
            bytes[0] &= !1;
            bytes[(width / 8) as usize] &= !(1 << outside_bit);
            let data = bytes.iter().map(|b| format!("i8 {b}")).collect::<Vec<_>>().join(", ");
            let text = format!(
                "memory 512\ndata {address} = {data}\nfunc @main() -> i{width} {{\nentry:\n\
                 h [{address}]\n h [{outside_address}:{outside_bit}]\n s [{outside_address}:{outside_bit}]\n\
                 %m = measure i{width} [{address}]\n ret %m\n}}"
            );
            let module = load(&text);
            let low = u128::from_le_bytes(bytes[..16].try_into().expect("sixteen bytes"));
            let mask = u128::MAX >> (128 - width);
            for seed in 0..4 {
                let mut e = executor(&module, "main", &[], Options { seed, reversible: true, ..bounded() });
                let Outcome::Returned(Some(value)) = e.run() else { panic!("i{width}: {:?}", e.outcome()) };
                assert_eq!(value & !1, low & mask, "i{width} at {address}");
                let state = e.snapshot();
                assert_eq!(state.quantum.amplitudes.len(), 2);
                for (config, amp) in &state.quantum.amplitudes {
                    let column = state.quantum.cells.iter().find(|&&(a, _)| a == outside_address).expect("cell").1;
                    let outside = config[column] >> outside_bit & 1;
                    let expected = if outside == 0 { C::new(0.5f64.sqrt(), 0.0) } else { C::new(0.0, 0.5f64.sqrt()) };
                    assert!((*amp - expected).norm() < 1e-12, "i{width}: {config:?}: {amp}");
                }
                assert_eq!(e.reverse(), Err(ReverseError::PastMeasurement));
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 1024);
}

#[test]
fn every_width_checks_its_last_byte_and_address_overflow() {
    for width in 1u64..=128 {
        let module = load(&format!(
            "memory 512\nfunc @main(i64 %p) -> i{width} {{\nentry:\n %v = measure i{width} [%p]\n ret %v\n}}"
        ));
        let len = width.div_ceil(8);
        for address in [512 - len, 513 - len, 512, u64::MAX - len + 1, u64::MAX] {
            let mut e = executor(&module, "main", &[address], bounded());
            let expected =
                if address == 512 - len { Outcome::Returned(Some(0)) } else { Outcome::Faulted(Fault::OutOfBounds) };
            assert_eq!(e.run(), expected, "i{width} at {address}");
        }
    }
}

/// Ry(pi/3) gives probabilities 3/4 and 1/4. Copy its bit into a second cell; that cell's
/// unmeasured bit 7 has equal weights and an independent phase. Marginalizing that bit must
/// neither bias the draw nor collapse it. Fixed seeds make this distribution check repeatable;
/// exact support and complex conditional states are checked independently of the frequency bound.
#[test]
fn joint_measurement_uses_born_weights_after_marginalizing_unmeasured_bits() {
    let module = load(
        "memory 512\nfunc @main() -> i9 {\nentry:\n ry(1.0471975511965976) [17]\n x [18:0] ctrl [17]\n h [18:7]\n s [18:7]\n %m = measure i9 [17]\n ret %m\n}",
    );
    let mut ones = 0;
    const SAMPLES: u64 = 4096;
    for seed in 0..SAMPLES {
        let mut e = executor(&module, "main", &[], Options { seed, ..bounded() });
        let Outcome::Returned(Some(value)) = e.run() else { panic!("{:?}", e.outcome()) };
        assert!([0, 257].contains(&value));
        ones += u64::from(value == 257);
        assert_eq!(e.snapshot().quantum.amplitudes.len(), 2);
    }
    let frequency = ones as f64 / SAMPLES as f64;
    assert!((frequency - 0.25).abs() < 0.03, "{ones}/{SAMPLES}");
}

#[test]
fn address_measurements_fault_inside_regions_in_all_apply_modes() {
    for width in [1, 7, 8, 9, 64, 65, 128] {
        let module = load(&format!(
            "memory 512\nfunc @read() -> i1 {{\nentry:\n %m = measure i{width} [17]\n ret 1\n}}\nfunc @main() {{\nentry:\n %out = qalloc 1\n x %out\n %f = qalloc 5\n x %f[0]\n apply @read() -> %out, %f budget 32\n ret\n}}"
        ));
        let mut e =
            executor(&module, "main", &[], Options { allow_region_faults: true, reversible: true, ..bounded() });
        let before = e.snapshot();
        assert_eq!(e.run(), Outcome::Returned(None));
        assert!((e.snapshot().quantum.amplitude(&[1, 9]).re - 1.0).abs() < 1e-12);
        e.reverse().expect("fault was before measurement");
        crate::common::assert_same_state(&e.snapshot(), &before, "region measurement reverses");
    }
}
