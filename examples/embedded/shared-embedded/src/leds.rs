use embassy_futures::select::{Either, select};
use embassy_stm32::gpio;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use types::led;

pub static LED_MODE: Signal<CriticalSectionRawMutex, led::Mode> = Signal::new();

const HEARTBEAT_PERIOD: Duration = Duration::from_millis(500);
const DEFAULT_LED_MODE: led::Mode =
    led::Mode::AlternatingToggle(core::time::Duration::from_millis(1000));

pub struct Leds {
    pub red: gpio::Output<'static>,
    pub orange: gpio::Output<'static>,
}

pub async fn heartbeat(led: &mut gpio::Output<'static>) {
    loop {
        led.toggle();
        Timer::after(HEARTBEAT_PERIOD).await;
    }
}

/// Applies the current mode to the red and orange LED. A new mode is applied immediately.
pub async fn led_task(leds: &mut Leds) {
    let mut mode = DEFAULT_LED_MODE;
    loop {
        let toggle_period = match mode {
            led::Mode::AllOff => {
                leds.red.set_low();
                leds.orange.set_low();
                None
            }
            led::Mode::RedOn => {
                leds.red.set_high();
                leds.orange.set_low();
                None
            }
            led::Mode::OrangeOn => {
                leds.red.set_low();
                leds.orange.set_high();
                None
            }
            led::Mode::AlternatingToggle(period) => {
                leds.red.toggle();
                leds.orange.set_level((!leds.red.is_set_high()).into());
                Some(period)
            }
            led::Mode::UnifiedToggle(period) => {
                leds.red.toggle();
                leds.orange.set_level(leds.red.is_set_high().into());
                Some(period)
            }
        };
        mode = match toggle_period {
            Some(period) => {
                let period = Duration::try_from(period).unwrap_or(Duration::MAX);
                match select(Timer::after(period), LED_MODE.wait()).await {
                    Either::First(()) => mode,
                    Either::Second(new_mode) => new_mode,
                }
            }
            None => LED_MODE.wait().await,
        };
    }
}
