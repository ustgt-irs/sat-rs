#![no_main]
#![no_std]

use panic_probe as _;
use rtic::app;

#[app(device = embassy_stm32)]
mod app {
    use embassy_time::Timer;
    use satrs_stm32f3_disco_rtic::{Direction, LedPinSet, Leds};

    #[shared]
    struct Shared {}

    #[local]
    struct Local {
        leds: Leds,
        current_dir: Direction,
    }

    #[init]
    fn init(_cx: init::Context) -> (Shared, Local) {
        let p = embassy_stm32::init(Default::default());

        defmt::info!("Starting sat-rs demo application for the STM32F3-Discovery using RTICv2");

        let led_pin_set = LedPinSet {
            pin_n: p.PE8,
            pin_ne: p.PE9,
            pin_e: p.PE10,
            pin_se: p.PE11,
            pin_s: p.PE12,
            pin_sw: p.PE13,
            pin_w: p.PE14,
            pin_nw: p.PE15,
        };
        let leds = Leds::new(led_pin_set);

        blinky::spawn().expect("failed to spawn blinky task");
        (
            Shared {},
            Local {
                leds,
                current_dir: Direction::North,
            },
        )
    }

    #[task(local = [leds, current_dir])]
    async fn blinky(cx: blinky::Context) {
        loop {
            cx.local.leds.blink_next(cx.local.current_dir);
            Timer::after_millis(200).await;
        }
    }
}
