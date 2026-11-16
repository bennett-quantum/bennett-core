//! Scalar Rust inputs: compile this as a library, or include it in the native oracle.
#![no_std]

#[no_mangle]
pub extern "C" fn arithmetic(a: u8, b: u8) -> u8 {
    a.wrapping_mul(17).wrapping_add(b).rotate_left(3) ^ b
}

#[no_mangle]
pub extern "C" fn branchy(a: u8) -> u8 {
    if a < 37 {
        a.wrapping_mul(3)
    } else if a & 1 == 0 {
        a.rotate_right(2) ^ 0xa5
    } else {
        a.wrapping_sub(91)
    }
}

#[no_mangle]
pub extern "C" fn counted_loop(a: u8) -> u8 {
    let mut value = a;
    let mut i = 0u8;
    while i < a {
        value = value.rotate_left(1).wrapping_add(i) ^ 0x5b;
        i = i.wrapping_add(1);
    }
    value
}

#[no_mangle]
pub extern "C" fn checked(a: u8, b: u8) -> u8 {
    match a.checked_add(b) {
        Some(sum) => sum ^ 0x55,
        None => a.wrapping_sub(b),
    }
}

// Debug overflow checks call core's panic routine; unchecked release builds wrap.
#[cfg(not(no_ordinary_add))]
#[no_mangle]
pub extern "C" fn ordinary_add(a: u8, b: u8) -> u8 {
    a + b
}
