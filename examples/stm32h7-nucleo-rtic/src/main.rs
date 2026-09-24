#![no_main]
#![no_std]
extern crate alloc;

use rtic::app;
// global logger + panicking-behavior + memory layout
use embassy_stm32::bind_interrupts;
use satrs_stm32h7_nucleo_rtic as _;

use core::mem::MaybeUninit;
use embedded_alloc::LlffHeap as Heap;

const DEFAULT_BLINK_FREQ_MS: u32 = 1000;
const PORT: u16 = 7301;

const HEAP_SIZE: usize = 131_072;

#[global_allocator]
static HEAP: Heap = Heap::empty();

/// Locally administered MAC address
const MAC_ADDRESS: [u8; 6] = [0x02, 0x00, 0x11, 0x22, 0x33, 0x44];

const TC_QUEUE_DEPTH: usize = 32;
const TM_QUEUE_DEPTH: usize = 32;

#[app(device = embassy_stm32, peripherals = false)]
mod app {

    use super::*;
    use arbitrary_int::u14;
    use embassy_net::udp::UdpSocket;
    use embassy_net::StackResources;
    use embassy_stm32::eth;
    use embassy_stm32::gpio;
    use embassy_stm32::peripherals;
    use embassy_stm32::rng;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embassy_time::Duration;
    use embassy_time::Timer;
    use embassy_time::WithTimeout as _;
    use embedded_types::create_tm_packet;
    use embedded_types::stm32h7;
    use embedded_types::tm_size;
    use embedded_types::TmHeader;
    use spacepackets::CcsdsPacketCreationError;
    use spacepackets::CcsdsPacketIdAndPsc;
    use spacepackets::CcsdsPacketReader;
    use spacepackets::SpHeader;
    use static_cell::StaticCell;

    bind_interrupts!(struct Irqs {
        ETH => eth::InterruptHandler;
        HASH_RNG => rng::InterruptHandler<peripherals::RNG>;
    });

    type Device = eth::Ethernet<
        'static,
        peripherals::ETH,
        eth::GenericPhy<eth::Sma<'static, peripherals::ETH_SMA>>,
    >;

    struct BlinkyLeds {
        led1: gpio::Output<'static>,
        led2: gpio::Output<'static>,
    }

    #[local]
    struct Local {
        net_runner: embassy_net::Runner<'static, Device>,
        net_stack: embassy_net::Stack<'static>,
        leds: BlinkyLeds,
        link_led: gpio::Output<'static>,
        tc_rx: embassy_sync::channel::Receiver<
            'static,
            NoopRawMutex,
            alloc::vec::Vec<u8>,
            TC_QUEUE_DEPTH,
        >,
        tc_tx: embassy_sync::channel::Sender<
            'static,
            NoopRawMutex,
            alloc::vec::Vec<u8>,
            TC_QUEUE_DEPTH,
        >,
        tm_rx: embassy_sync::channel::Receiver<
            'static,
            NoopRawMutex,
            alloc::vec::Vec<u8>,
            TM_QUEUE_DEPTH,
        >,
        tm_tx: embassy_sync::channel::Sender<
            'static,
            NoopRawMutex,
            alloc::vec::Vec<u8>,
            TM_QUEUE_DEPTH,
        >,
    }

    #[shared]
    struct Shared {
        sequence_count: u14,
        blink_freq: embassy_time::Duration,
    }

    #[init]
    fn init(_cx: init::Context) -> (Shared, Local) {
        defmt::println!("Starting sat-rs demo application for the STM32H753ZIT");

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

        let link_led = gpio::Output::new(periphs.PB0, gpio::Level::Low, gpio::Speed::Medium);
        let mut led1 = gpio::Output::new(periphs.PE1, gpio::Level::Low, gpio::Speed::Medium);
        let mut led2 = gpio::Output::new(periphs.PB14, gpio::Level::Low, gpio::Speed::Medium);

        // Criss-cross pattern looks cooler.
        led1.set_high();
        led2.set_low();
        let leds = BlinkyLeds { led1, led2 };

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
            MAC_ADDRESS,
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

        // Init network stack
        static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
        let (stack, runner) =
            embassy_net::new(device, config, RESOURCES.init(StackResources::new()), seed);

        // Set up global allocator. Use AXISRAM for the heap.
        #[link_section = ".axisram"]
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { HEAP.init(&raw mut HEAP_MEM as usize, HEAP_SIZE) }

        static TC_CHANNEL: static_cell::ConstStaticCell<
            embassy_sync::channel::Channel<NoopRawMutex, alloc::vec::Vec<u8>, TC_QUEUE_DEPTH>,
        > = static_cell::ConstStaticCell::new(embassy_sync::channel::Channel::new());
        let tc_channel = TC_CHANNEL.take();
        let tc_sender = tc_channel.sender();
        let tc_receiver = tc_channel.receiver();

        static TM_CHANNEL: static_cell::ConstStaticCell<
            embassy_sync::channel::Channel<NoopRawMutex, alloc::vec::Vec<u8>, TM_QUEUE_DEPTH>,
        > = static_cell::ConstStaticCell::new(embassy_sync::channel::Channel::new());
        let tm_channel = TM_CHANNEL.take();
        let tm_sender = tm_channel.sender();
        let tm_receiver = tm_channel.receiver();

        net_lib_task::spawn().expect("spawning net library task failed");
        net_app_task::spawn().expect("spawning net application task failed");
        blinky::spawn().expect("spawning blink task failed");
        tc_handler::spawn().expect("spawning TC handler task failed");

        (
            Shared {
                blink_freq: Duration::from_millis(DEFAULT_BLINK_FREQ_MS as u64),
                sequence_count: u14::new(0),
            },
            Local {
                link_led,
                leds,
                net_runner: runner,
                net_stack: stack,
                tc_tx: tc_sender,
                tc_rx: tc_receiver,
                tm_tx: tm_sender,
                tm_rx: tm_receiver,
            },
        )
    }

    #[task(local = [leds], shared=[blink_freq])]
    async fn blinky(mut cx: blinky::Context) {
        let leds = cx.local.leds;
        loop {
            leds.led1.toggle();
            leds.led2.toggle();
            let current_blink_freq = cx.shared.blink_freq.lock(|current| *current);
            Timer::after_millis(current_blink_freq.as_millis()).await;
        }
    }

    #[task(local=[net_runner])]
    async fn net_lib_task(cx: net_lib_task::Context) {
        cx.local.net_runner.run().await;
    }

    #[task(local = [net_stack, link_led, tc_tx, tm_rx])]
    async fn net_app_task(cx: net_app_task::Context) {
        pub const MTU: usize = 1500;

        // Ensure those are in the data section by making them static.
        static RX_UDP_META: static_cell::ConstStaticCell<[embassy_net::udp::PacketMetadata; 8]> =
            static_cell::ConstStaticCell::new([embassy_net::udp::PacketMetadata::EMPTY; 8]);
        static TX_UDP_META: static_cell::ConstStaticCell<[embassy_net::udp::PacketMetadata; 8]> =
            static_cell::ConstStaticCell::new([embassy_net::udp::PacketMetadata::EMPTY; 8]);
        static TX_UDP_BUFS: static_cell::ConstStaticCell<[u8; MTU]> =
            static_cell::ConstStaticCell::new([0; MTU]);
        static RX_UDP_BUFS: static_cell::ConstStaticCell<[u8; MTU]> =
            static_cell::ConstStaticCell::new([0; MTU]);

        let rx_udp_meta = RX_UDP_META.take();
        let rx_udp_bufs = RX_UDP_BUFS.take();
        let tx_udp_meta = TX_UDP_META.take();
        let tx_udp_bufs = TX_UDP_BUFS.take();

        let mut rx_buffer = [0; MTU];

        loop {
            cx.local.net_stack.wait_link_up().await;
            cx.local.link_led.set_high();
            defmt::info!("Network link is up");

            // Ensure DHCP configuration is up before trying connect
            cx.local.net_stack.wait_config_up().await;

            let config = cx.local.net_stack.config_v4();
            defmt::info!("Network task initialized, config: {}", config);

            let mut udp = UdpSocket::new(
                cx.local.net_stack.clone(),
                rx_udp_meta,
                rx_udp_bufs,
                tx_udp_meta,
                tx_udp_bufs,
            );
            defmt::info!("UDP socket bound to port {}", PORT);
            udp.bind(PORT).expect("failed to bind UDP socket");
            let mut remote_endpoint = None;
            loop {
                if !cx.local.net_stack.is_link_up() {
                    defmt::warn!("Network link is down");
                    cx.local.link_led.set_low();
                    break;
                }
                match udp
                    .recv_from(&mut rx_buffer)
                    .with_timeout(Duration::from_millis(200))
                    .await
                {
                    Ok(result) => match result {
                        Ok((data, meta)) => {
                            remote_endpoint = Some(meta.endpoint);
                            defmt::debug!("UDP RX {}, Meta: {}", data, meta);
                            cx.local.tc_tx.send(rx_buffer[0..data].to_vec()).await;
                        }
                        Err(e) => {
                            defmt::warn!("udp receive error: {}", e);
                            Timer::after_millis(100).await;
                        }
                    },
                    Err(_e) => (),
                }
                if let Some(endpoint) = remote_endpoint {
                    while let Ok(packet) = cx.local.tm_rx.try_receive() {
                        match udp.send_to(&packet, endpoint).await {
                            Ok(_) => {
                                defmt::debug!("UDP TX: {} bytes to: {}", packet.len(), endpoint)
                            }
                            Err(e) => defmt::warn!("udp send error: {}", e),
                        }
                    }
                }
            }
        }
    }

    #[task(local = [tc_rx, tm_tx], shared=[sequence_count, blink_freq])]
    async fn tc_handler(mut cx: tc_handler::Context) {
        loop {
            let tc = cx.local.tc_rx.receive().await;

            match CcsdsPacketReader::new_with_checksum(&tc) {
                Ok(packet) => {
                    let packet_id = packet.packet_id();
                    let psc = packet.psc();
                    let tc_packet_id = CcsdsPacketIdAndPsc { packet_id, psc };
                    if let Ok(request) =
                        postcard::from_bytes::<stm32h7::Request>(packet.packet_data())
                    {
                        let response = match request {
                            stm32h7::Request::Ping => {
                                defmt::info!("Received Ping request");
                                stm32h7::Response::Ok
                            }
                            stm32h7::Request::ChangeBlinkFrequency(duration) => {
                                defmt::info!(
                                    "Received blinky frequency change request: {} ms",
                                    duration.as_millis()
                                );
                                cx.shared.blink_freq.lock(|current| {
                                    *current = Duration::from_millis(duration.as_millis() as u64)
                                });
                                stm32h7::Response::Ok
                            }
                        };
                        let sequence_count = cx.shared.sequence_count.lock(|v| {
                            let current = *v;
                            *v = v.wrapping_add(u14::new(1));
                            current
                        });

                        // Send Pong/OK response immediately.
                        if let Err(e) =
                            send_tm(tc_packet_id, response, sequence_count, cx.local.tm_tx).await
                        {
                            defmt::warn!("Failed to send TM response: {}", e);
                        }
                    }
                }
                Err(e) => defmt::warn!("Failed to parse received TC packet: {}", e,),
            }
            defmt::info!("Received from UDP client: {}", tc.as_slice());
        }
    }

    async fn send_tm(
        tc_packet_id: CcsdsPacketIdAndPsc,
        response: stm32h7::Response,
        current_seq_count: u14,
        sender: &embassy_sync::channel::Sender<
            'static,
            NoopRawMutex,
            alloc::vec::Vec<u8>,
            TM_QUEUE_DEPTH,
        >,
    ) -> Result<(), CcsdsPacketCreationError> {
        let sp_header = SpHeader::new_for_unseg_tc(stm32h7::PUS_APID, current_seq_count, 0);
        let tm_header = TmHeader {
            tc_packet_id: Some(tc_packet_id),
            uptime_millis: embassy_time::Instant::now().as_millis(),
        };
        let tm_size = tm_size(&tm_header, &response);
        let mut packet = alloc::vec![0; tm_size];
        create_tm_packet(&mut packet, sp_header, tm_header, response)?;
        sender.send(packet).await;
        Ok(())
    }
}
