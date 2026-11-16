//! Optional no_std final link: retain core's real overflow checks and panic calls.
#![no_std]
#[path = "scalars.rs"]
mod scalars;

extern "C" {
    fn abort() -> !;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // Use the ordinary abort boundary; the experiment links a trap implementation
    // for the device and libc's abort when executing natively.
    unsafe { abort() }
}
