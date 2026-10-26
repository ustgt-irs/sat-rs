#![no_main]
#![no_std]
extern crate alloc;

use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_stm32::{bind_interrupts, eth, gpio, peripherals, rng};
use static_cell::StaticCell;

bind_interrupts!(struct Irqs {
    ETH => eth::InterruptHandler;
    HASH_RNG => rng::InterruptHandler<peripherals::RNG>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::println!("Starting sat-rs demo application for the STM32H753ZIT");

    // Safety: Called once, before the first allocation.
    unsafe { stm32h7_nucleo_embassy::init_heap() };

    let mut config = embassy_stm32::Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hsi = Some(HSIPrescaler::DIV1);
        config.rcc.csi = true;
        config.rcc.hsi48 = Some(Default::default()); // needed for RNG
        config.rcc.pll1 = Some(Pll {
            source: PllSource::HSI,
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL50,
            divp: Some(PllDiv::DIV2),
            divq: None,
            divr: None,
        });
        config.rcc.sys = Sysclk::PLL1_P; // 400 Mhz
        config.rcc.ahb_pre = AHBPrescaler::DIV2; // 200 Mhz
        config.rcc.apb1_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb2_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb3_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.apb4_pre = APBPrescaler::DIV2; // 100 Mhz
        config.rcc.voltage_scale = VoltageScale::Scale1;
    }
    let periphs = embassy_stm32::init(config);

    let green_led = gpio::Output::new(periphs.PB0, gpio::Level::Low, gpio::Speed::Medium);
    let leds = shared_embedded::leds::Leds {
        red: gpio::Output::new(periphs.PB14, gpio::Level::Low, gpio::Speed::Medium),
        orange: gpio::Output::new(periphs.PE1, gpio::Level::Low, gpio::Speed::Medium),
    };

    static PACKETS: StaticCell<eth::PacketQueue<4, 4>> = StaticCell::new();
    // warning: Not all STM32H7 devices have the exact same pins here
    // for STM32H747XIH, replace p.PB13 for PG12
    let device = eth::Ethernet::new(
        PACKETS.init(eth::PacketQueue::<4, 4>::new()),
        periphs.ETH,
        Irqs,
        periphs.PA1,  // ref_clk
        periphs.PA7,  // CRS_DV: Carrier Sense
        periphs.PC4,  // RX_D0: Received Bit 0
        periphs.PC5,  // RX_D1: Received Bit 1
        periphs.PG13, // TX_D0: Transmit Bit 0
        periphs.PB13, // TX_D1: Transmit Bit 1
        periphs.PG11, // TX_EN: Transmit Enable
        shared_embedded::net::MAC_ADDRESS,
        periphs.ETH_SMA,
        periphs.PA2, // mdio
        periphs.PC1, // mdc
    );

    let net_config = embassy_net::Config::dhcpv4(embassy_net::DhcpConfig::default());

    // Generate random seed.
    let mut rng = rng::Rng::new(periphs.RNG, Irqs);
    let mut seed = [0; 8];
    rng.fill_bytes(&mut seed);
    let seed = u64::from_le_bytes(seed);

    // DHCP, TMTC and simulator socket.
    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    static CHANNELS: shared_embedded::channels::Channels =
        shared_embedded::channels::Channels::new();
    static SHARED_SWITCH_SET: StaticCell<shared_embedded::pcdu::SharedSwitchSet> =
        StaticCell::new();
    let shared_switch_set = SHARED_SWITCH_SET.init(shared_embedded::pcdu::new_shared_switch_set());
    let pcdu_handler = shared_embedded::pcdu::PcduHandler::new(CHANNELS.pcdu(), shared_switch_set);

    spawner.spawn(net_stack_task(runner).expect("spawning net stack task failed"));
    spawner.spawn(
        sim_client_task(stack, CHANNELS.sim_client()).expect("spawning sim client task failed"),
    );
    spawner.spawn(udp_task(stack, CHANNELS.udp()).expect("spawning UDP task failed"));
    spawner.spawn(heartbeat(green_led).expect("spawning heartbeat task failed"));
    spawner.spawn(heap_monitor().expect("spawning heap monitor task failed"));
    spawner.spawn(led_task(leds).expect("spawning LED task failed"));
    spawner.spawn(tc_handler(CHANNELS.tc_handler()).expect("spawning TC handler task failed"));
    spawner.spawn(tm_funnel(CHANNELS.tm_funnel()).expect("spawning TM funnel task failed"));
    spawner.spawn(
        event_manager(CHANNELS.event_manager()).expect("spawning event manager task failed"),
    );
    spawner.spawn(pcdu_task(pcdu_handler).expect("spawning PCDU task failed"));
}

#[embassy_executor::task]
async fn net_stack_task(
    mut runner: embassy_net::Runner<'static, shared_embedded::net::Device>,
) -> ! {
    shared_embedded::net::net_stack_task(&mut runner).await
}

#[embassy_executor::task]
async fn udp_task(
    stack: embassy_net::Stack<'static>,
    endpoints: shared_embedded::net::UdpEndpoints,
) {
    shared_embedded::net::udp_task(stack, endpoints).await
}

#[embassy_executor::task]
async fn sim_client_task(
    stack: embassy_net::Stack<'static>,
    endpoints: shared_embedded::sim_client::SimClientEndpoints,
) {
    shared_embedded::sim_client::sim_client_task(stack, endpoints).await
}

#[embassy_executor::task]
async fn heartbeat(mut led: gpio::Output<'static>) {
    shared_embedded::leds::heartbeat(&mut led).await
}

#[embassy_executor::task]
async fn heap_monitor() {
    shared_embedded::heap::heap_monitor_task(
        stm32h7_nucleo_embassy::heap_used,
        stm32h7_nucleo_embassy::HEAP_SIZE,
    )
    .await
}

#[embassy_executor::task]
async fn led_task(mut leds: shared_embedded::leds::Leds) {
    shared_embedded::leds::led_task(&mut leds).await
}

#[embassy_executor::task]
async fn tc_handler(endpoints: shared_embedded::tmtc::TcHandlerEndpoints) {
    shared_embedded::tmtc::tc_handler(endpoints).await
}

#[embassy_executor::task]
async fn tm_funnel(endpoints: shared_embedded::tmtc::TmFunnelEndpoints) {
    shared_embedded::tmtc::tm_funnel_task(endpoints).await
}

#[embassy_executor::task]
async fn event_manager(endpoints: shared_embedded::event_manager::EventManagerEndpoints) {
    shared_embedded::event_manager::event_manager_task(endpoints).await
}

#[embassy_executor::task]
async fn pcdu_task(mut handler: shared_embedded::pcdu::PcduHandler) {
    shared_embedded::pcdu::pcdu_task(&mut handler).await
}
