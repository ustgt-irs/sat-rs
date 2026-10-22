#![no_main]
#![no_std]
extern crate alloc;

use rtic::app;

#[app(device = embassy_stm32, peripherals = false)]
mod app {
    use embassy_net::StackResources;
    use embassy_stm32::{bind_interrupts, eth, gpio, peripherals, rng};
    use static_cell::StaticCell;

    bind_interrupts!(struct Irqs {
        ETH => eth::InterruptHandler;
        HASH_RNG => rng::InterruptHandler<peripherals::RNG>;
    });

    #[local]
    struct Local {
        net_runner: embassy_net::Runner<'static, shared_embedded::net::Device>,
        net_stack: embassy_net::Stack<'static>,
        sim_net_stack: embassy_net::Stack<'static>,
        leds: shared_embedded::leds::Leds,
        green_led: gpio::Output<'static>,
        udp_endpoints: shared_embedded::net::UdpEndpoints,
        sim_client_endpoints: shared_embedded::sim_client::SimClientEndpoints,
        tc_handler_endpoints: shared_embedded::tmtc::TcHandlerEndpoints,
        tm_funnel_endpoints: shared_embedded::tmtc::TmFunnelEndpoints,
        event_manager_endpoints: shared_embedded::event_manager::EventManagerEndpoints,
        pcdu_handler: shared_embedded::pcdu::PcduHandler,
    }

    #[shared]
    struct Shared {}

    #[init]
    fn init(_cx: init::Context) -> (Shared, Local) {
        defmt::println!("Starting sat-rs demo application for the STM32H753ZIT");

        // Safety: Called once, before the first allocation.
        unsafe { stm32h7_nucleo_rtic::init_heap() };

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

        let config = embassy_net::Config::dhcpv4(embassy_net::DhcpConfig::default());

        // Generate random seed.
        let mut rng = rng::Rng::new(periphs.RNG, Irqs);
        let mut seed = [0; 8];
        rng.fill_bytes(&mut seed);
        let seed = u64::from_le_bytes(seed);

        // DHCP, TMTC and simulator socket.
        static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
        let (stack, runner) =
            embassy_net::new(device, config, RESOURCES.init(StackResources::new()), seed);

        static CHANNELS: shared_embedded::channels::Channels =
            shared_embedded::channels::Channels::new();
        static SHARED_SWITCH_SET: StaticCell<shared_embedded::pcdu::SharedSwitchSet> =
            StaticCell::new();
        let shared_switch_set =
            SHARED_SWITCH_SET.init(shared_embedded::pcdu::new_shared_switch_set());
        let pcdu_handler =
            shared_embedded::pcdu::PcduHandler::new(CHANNELS.pcdu(), shared_switch_set);

        net_lib_task::spawn().expect("spawning net library task failed");
        udp_task::spawn().expect("spawning UDP task failed");
        sim_client_task::spawn().expect("spawning sim client task failed");
        heartbeat::spawn().expect("spawning heartbeat task failed");
        heap_monitor::spawn().expect("spawning heap monitor task failed");
        led_task::spawn().expect("spawning LED task failed");
        tc_handler::spawn().expect("spawning TC handler task failed");
        tm_funnel::spawn().expect("spawning TM funnel task failed");
        event_manager::spawn().expect("spawning event manager task failed");
        pcdu_task::spawn().expect("spawning PCDU task failed");

        (
            Shared {},
            Local {
                green_led,
                leds,
                net_runner: runner,
                net_stack: stack,
                sim_net_stack: stack,
                udp_endpoints: CHANNELS.udp(),
                sim_client_endpoints: CHANNELS.sim_client(),
                tc_handler_endpoints: CHANNELS.tc_handler(),
                tm_funnel_endpoints: CHANNELS.tm_funnel(),
                event_manager_endpoints: CHANNELS.event_manager(),
                pcdu_handler,
            },
        )
    }

    #[task(local = [green_led])]
    async fn heartbeat(cx: heartbeat::Context) {
        shared_embedded::leds::heartbeat(cx.local.green_led).await;
    }

    #[task]
    async fn heap_monitor(_cx: heap_monitor::Context) {
        shared_embedded::heap::heap_monitor_task(
            stm32h7_nucleo_rtic::heap_used,
            stm32h7_nucleo_rtic::HEAP_SIZE,
        )
        .await;
    }

    #[task(local = [leds])]
    async fn led_task(cx: led_task::Context) {
        shared_embedded::leds::led_task(cx.local.leds).await;
    }

    #[task(local = [net_runner])]
    async fn net_lib_task(cx: net_lib_task::Context) {
        shared_embedded::net::net_stack_task(cx.local.net_runner).await;
    }

    #[task(local = [net_stack, udp_endpoints])]
    async fn udp_task(cx: udp_task::Context) {
        shared_embedded::net::udp_task(*cx.local.net_stack, *cx.local.udp_endpoints).await;
    }

    #[task(local = [sim_net_stack, sim_client_endpoints])]
    async fn sim_client_task(cx: sim_client_task::Context) {
        shared_embedded::sim_client::sim_client_task(
            *cx.local.sim_net_stack,
            *cx.local.sim_client_endpoints,
        )
        .await;
    }

    #[task(local = [tc_handler_endpoints])]
    async fn tc_handler(cx: tc_handler::Context) {
        shared_embedded::tmtc::tc_handler(*cx.local.tc_handler_endpoints).await;
    }

    #[task(local = [tm_funnel_endpoints])]
    async fn tm_funnel(cx: tm_funnel::Context) {
        shared_embedded::tmtc::tm_funnel_task(*cx.local.tm_funnel_endpoints).await;
    }

    #[task(local = [event_manager_endpoints])]
    async fn event_manager(cx: event_manager::Context) {
        shared_embedded::event_manager::event_manager_task(*cx.local.event_manager_endpoints).await;
    }

    #[task(local = [pcdu_handler])]
    async fn pcdu_task(cx: pcdu_task::Context) {
        shared_embedded::pcdu::pcdu_task(cx.local.pcdu_handler).await;
    }
}
