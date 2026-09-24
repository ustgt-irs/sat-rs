#![no_main]
#![no_std]

// global logger + panicking-behavior + memory layout
use satrs_stm32h7_nucleo_rtic as _;

#[cortex_m_rt::entry]
fn main() -> ! {
    loop {
        defmt::println!("Hello, world!");
        cortex_m::asm::delay(100_000_000);
    }
}
