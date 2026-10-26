#![no_main]
#![no_std]
extern crate alloc;

use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_stm32::{bind_interrupts, eth, gpio, peripherals, rng};
use static_cell::{ConstStaticCell, StaticCell};

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

    static TC_CHANNEL: ConstStaticCell<shared_embedded::tmtc::TcChannel> =
        ConstStaticCell::new(shared_embedded::tmtc::TcChannel::new());
    let tc_channel = TC_CHANNEL.take();
    static TM_CHANNEL: ConstStaticCell<shared_embedded::tmtc::TmChannel> =
        ConstStaticCell::new(shared_embedded::tmtc::TmChannel::new());
    let tm_channel = TM_CHANNEL.take();
    static TM_FUNNEL_CHANNEL: ConstStaticCell<shared_embedded::tmtc::TmFunnelChannel> =
        ConstStaticCell::new(shared_embedded::tmtc::TmFunnelChannel::new());
    let tm_funnel_channel = TM_FUNNEL_CHANNEL.take();
    static EVENT_CHANNEL: ConstStaticCell<shared_embedded::event_manager::EventChannel> =
        ConstStaticCell::new(shared_embedded::event_manager::EventChannel::new());
    let event_channel = EVENT_CHANNEL.take();
    static SIM_REQUEST_CHANNEL: ConstStaticCell<shared_embedded::sim_client::SimRequestChannel> =
        ConstStaticCell::new(shared_embedded::sim_client::SimRequestChannel::new());
    let sim_request_channel = SIM_REQUEST_CHANNEL.take();
    static PCDU_SIM_REPLY_CHANNEL: ConstStaticCell<
        shared_embedded::sim_client::PcduSimReplyChannel,
    > = ConstStaticCell::new(shared_embedded::sim_client::PcduSimReplyChannel::new());
    let pcdu_sim_reply_channel = PCDU_SIM_REPLY_CHANNEL.take();
    static PCDU_TC_CHANNEL: ConstStaticCell<shared_embedded::pcdu::PcduTcChannel> =
        ConstStaticCell::new(shared_embedded::pcdu::PcduTcChannel::new());
    let pcdu_tc_channel = PCDU_TC_CHANNEL.take();
    static PCDU_SWITCH_REQUEST_CHANNEL: ConstStaticCell<
        shared_embedded::pcdu::PcduSwitchRequestChannel,
    > = ConstStaticCell::new(shared_embedded::pcdu::PcduSwitchRequestChannel::new());
    let pcdu_switch_request_channel = PCDU_SWITCH_REQUEST_CHANNEL.take();
    static SHARED_SWITCH_SET: StaticCell<shared_embedded::pcdu::SharedSwitchSet> =
        StaticCell::new();
    let shared_switch_set = SHARED_SWITCH_SET.init(shared_embedded::pcdu::new_shared_switch_set());
    // No component sends switch requests yet, so the sender is not used.
    let pcdu_handler = shared_embedded::pcdu::PcduHandler::new(
        pcdu_tc_channel.receiver(),
        tm_funnel_channel.sender(),
        pcdu_switch_request_channel.receiver(),
        shared_embedded::pcdu::PcduCommunication::Dummy(Default::default()),
        shared_switch_set,
        event_channel.sender(),
    );

    spawner.spawn(net_stack_task(runner).expect("spawning net stack task failed"));
    spawner.spawn(
        sim_client_task(
            stack,
            sim_request_channel.receiver(),
            shared_embedded::sim_client::SimReplySenders {
                pcdu: pcdu_sim_reply_channel.sender(),
            },
        )
        .expect("spawning sim client task failed"),
    );
    spawner.spawn(
        udp_task(stack, tc_channel.sender(), tm_channel.receiver())
            .expect("spawning UDP task failed"),
    );
    spawner.spawn(heartbeat(green_led).expect("spawning heartbeat task failed"));
    spawner.spawn(heap_monitor().expect("spawning heap monitor task failed"));
    spawner.spawn(led_task(leds).expect("spawning LED task failed"));
    spawner.spawn(
        tc_handler(
            tc_channel.receiver(),
            shared_embedded::tmtc::Telemetry::new(tm_funnel_channel.sender()),
            event_channel.sender(),
            shared_embedded::tmtc::TcSenders {
                pcdu: pcdu_tc_channel.sender(),
            },
        )
        .expect("spawning TC handler task failed"),
    );
    spawner.spawn(
        tm_funnel(tm_funnel_channel.receiver(), tm_channel.sender())
            .expect("spawning TM funnel task failed"),
    );
    spawner.spawn(
        event_manager(event_channel.receiver(), tm_funnel_channel.sender())
            .expect("spawning event manager task failed"),
    );
    spawner.spawn(
        pcdu_task(
            pcdu_handler,
            sim_request_channel.sender(),
            pcdu_sim_reply_channel.receiver(),
        )
        .expect("spawning PCDU task failed"),
    );
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
    tc_tx: shared_embedded::tmtc::TcSender,
    tm_rx: shared_embedded::tmtc::TmReceiver,
) {
    shared_embedded::net::udp_task(stack, tc_tx, tm_rx).await
}

#[embassy_executor::task]
async fn sim_client_task(
    stack: embassy_net::Stack<'static>,
    request_rx: shared_embedded::sim_client::SimRequestReceiver,
    reply_senders: shared_embedded::sim_client::SimReplySenders,
) {
    shared_embedded::sim_client::sim_client_task(stack, request_rx, reply_senders).await
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
async fn tc_handler(
    tc_rx: shared_embedded::tmtc::TcReceiver,
    mut telemetry: shared_embedded::tmtc::Telemetry,
    event_tx: shared_embedded::event_manager::EventSender,
    tc_senders: shared_embedded::tmtc::TcSenders,
) {
    shared_embedded::tmtc::tc_handler(tc_rx, &mut telemetry, event_tx, tc_senders).await
}

#[embassy_executor::task]
async fn tm_funnel(
    funnel_rx: shared_embedded::tmtc::TmFunnelReceiver,
    tm_tx: shared_embedded::tmtc::TmSender,
) {
    shared_embedded::tmtc::tm_funnel_task(funnel_rx, tm_tx).await
}

#[embassy_executor::task]
async fn event_manager(
    event_rx: shared_embedded::event_manager::EventReceiver,
    tm_tx: shared_embedded::tmtc::TmFunnelSender,
) {
    shared_embedded::event_manager::event_manager_task(event_rx, tm_tx).await
}

#[embassy_executor::task]
async fn pcdu_task(
    mut handler: shared_embedded::pcdu::PcduHandler,
    sim_request_tx: shared_embedded::sim_client::SimRequestSender,
    sim_reply_rx: shared_embedded::sim_client::PcduSimReplyReceiver,
) {
    shared_embedded::pcdu::pcdu_task(&mut handler, sim_request_tx, sim_reply_rx).await
}
