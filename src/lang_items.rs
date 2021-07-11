use crate::println;
use core::panic::PanicInfo;
use qemu_exit::QEMUExit;

#[lang = "eh_personality"]
pub extern "C" fn eh_personality() {}

#[panic_handler]
fn panic_handler(info: &PanicInfo) -> ! {
    println!("+++ Bold Kernel v{} Panic! +++", env!("CARGO_PKG_VERSION"));
    if let Some(message) = info.message() {
        println!("{}", message);
    }
    if let Some(location) = info.location() {
        println!("at {}", location);
    }
    println!("--- Bold Kernel v{} Panic! ---", env!("CARGO_PKG_VERSION"));
    loop {}
    // qemu_exit::AArch64::new().exit(1)
    // poweroff(false);
}
