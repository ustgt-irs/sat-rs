//! Blinks an LED
//!
//! This assumes that LD1 (green) is connected to PB0, LD2 (yellow) to PE1 and LD3 (red) to
//! PB14. This assumption is true for the MB1364 Nucleo-144 board, for example the
//! NUCLEO-H753ZI.

#![no_std]
#![no_main]
use rtic::app;
use satrs_stm32h7_nucleo_rtic as _;

#[app(device = embassy_stm32, peripherals = false, dispatchers = [SPI1])]
mod app {
    use embassy_stm32::gpio;

    #[shared]
    struct Shared {}

    #[local]
    struct Local {}

    #[init]
    fn init(_cx: init::Context) -> (Shared, Local) {
        let p = embassy_stm32::init(Default::default());
        defmt::info!("Hello World!");
        // Configure gpio B pin 0 as a push-pull output.
        let ld1 = gpio::Output::new(p.PB0, gpio::Level::High, gpio::Speed::Low);
        let ld2 = gpio::Output::new(p.PE1, gpio::Level::High, gpio::Speed::Low);
        let ld3 = gpio::Output::new(p.PB14, gpio::Level::High, gpio::Speed::Low);

        // Schedule the blinking task
        blink::spawn(ld1, ld2, ld3).ok();

        (Shared {}, Local {})
    }

    #[task()]
    async fn blink(
        _cx: blink::Context,
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
}
