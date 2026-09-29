//! Blinks an LED
//!
//! This assumes that LD1 (green) is connected to PB0, LD2 (yellow) to PE1 and LD3 (red) to
//! PB14. This assumption is true for the MB1364 Nucleo-144 board, for example the
//! NUCLEO-H753ZI.

#![no_std]
#![no_main]
use embassy_executor::Spawner;
use embassy_stm32::gpio;
use stm32h7_nucleo_embassy as _;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());
    defmt::info!("Hello World!");
    let ld1 = gpio::Output::new(p.PB0, gpio::Level::High, gpio::Speed::Low);
    let ld2 = gpio::Output::new(p.PE1, gpio::Level::High, gpio::Speed::Low);
    let ld3 = gpio::Output::new(p.PB14, gpio::Level::High, gpio::Speed::Low);

    spawner.spawn(blink(ld1, ld2, ld3).expect("spawning blink task failed"));
}

#[embassy_executor::task]
async fn blink(
    mut ld1: gpio::Output<'static>,
    mut ld2: gpio::Output<'static>,
    mut ld3: gpio::Output<'static>,
) {
    loop {
        defmt::info!("high");
        ld1.set_high();
        ld2.set_high();
        ld3.set_high();
        embassy_time::Timer::after_millis(500).await;

        defmt::info!("low");
        ld1.set_low();
        ld2.set_low();
        ld3.set_low();
        embassy_time::Timer::after_millis(500).await;
    }
}
