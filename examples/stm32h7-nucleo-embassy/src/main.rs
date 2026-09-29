#![no_main]
#![no_std]
extern crate alloc;

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

use alloc::vec::Vec;
use arbitrary_int::u14;
use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_net::StackResources;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_stm32::{bind_interrupts, eth, gpio, peripherals, rng};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::Timer;
use embedded_alloc::LlffHeap as Heap;
use embedded_types::{TmHeader, create_tm_packet, stm32h7, tm_size};
use spacepackets::{CcsdsPacketCreationError, CcsdsPacketIdAndPsc, CcsdsPacketReader, SpHeader};
use static_cell::{ConstStaticCell, StaticCell};
// global logger + panicking-behavior + memory layout
use stm32h7_nucleo_embassy as _;

const DEFAULT_BLINK_FREQ_MS: u32 = 1000;
const PORT: u16 = 7301;
const MTU: usize = 1500;

const HEAP_SIZE: usize = 131_072;

#[global_allocator]
static HEAP: Heap = Heap::empty();

/// Locally administered MAC address
const MAC_ADDRESS: [u8; 6] = [0x02, 0x00, 0x11, 0x22, 0x33, 0x44];

const TC_QUEUE_DEPTH: usize = 32;
const TM_QUEUE_DEPTH: usize = 32;

static BLINK_FREQ_MS: AtomicU32 = AtomicU32::new(DEFAULT_BLINK_FREQ_MS);

bind_interrupts!(struct Irqs {
    ETH => eth::InterruptHandler;
    HASH_RNG => rng::InterruptHandler<peripherals::RNG>;
});

type Device = eth::Ethernet<
    'static,
    peripherals::ETH,
    eth::GenericPhy<eth::Sma<'static, peripherals::ETH_SMA>>,
>;

type TcSender = Sender<'static, NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
type TcReceiver = Receiver<'static, NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
type TmSender = Sender<'static, NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;
type TmReceiver = Receiver<'static, NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;

struct BlinkyLeds {
    led1: gpio::Output<'static>,
    led2: gpio::Output<'static>,
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::println!("Starting sat-rs demo application for the STM32H753ZIT");

    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { HEAP.init(&raw mut HEAP_MEM as usize, HEAP_SIZE) }

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
    // Criss-cross pattern looks cooler.
    let leds = BlinkyLeds {
        led1: gpio::Output::new(periphs.PE1, gpio::Level::High, gpio::Speed::Medium),
        led2: gpio::Output::new(periphs.PB14, gpio::Level::Low, gpio::Speed::Medium),
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
        MAC_ADDRESS,
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

    static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        device,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    static TC_CHANNEL: ConstStaticCell<Channel<NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>> =
        ConstStaticCell::new(Channel::new());
    let tc_channel = TC_CHANNEL.take();
    static TM_CHANNEL: ConstStaticCell<Channel<NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>> =
        ConstStaticCell::new(Channel::new());
    let tm_channel = TM_CHANNEL.take();

    spawner.spawn(net_stack_task(runner).expect("spawning net stack task failed"));
    spawner.spawn(
        udp_task(stack, link_led, tc_channel.sender(), tm_channel.receiver())
            .expect("spawning UDP task failed"),
    );
    spawner.spawn(blinky(leds).expect("spawning blinky task failed"));
    spawner.spawn(
        tc_handler(tc_channel.receiver(), tm_channel.sender())
            .expect("spawning TC handler task failed"),
    );
}

#[embassy_executor::task]
async fn blinky(mut leds: BlinkyLeds) {
    loop {
        leds.led1.toggle();
        leds.led2.toggle();
        Timer::after_millis(BLINK_FREQ_MS.load(Ordering::Relaxed) as u64).await;
    }
}

#[embassy_executor::task]
async fn net_stack_task(mut runner: embassy_net::Runner<'static, Device>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn udp_task(
    stack: embassy_net::Stack<'static>,
    mut link_led: gpio::Output<'static>,
    tc_tx: TcSender,
    tm_rx: TmReceiver,
) {
    // Task futures are allocated statically, so these buffers do not live on the stack.
    let mut rx_udp_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_udp_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_udp_buf = [0; MTU];
    let mut tx_udp_buf = [0; MTU];
    let mut rx_buffer = [0; MTU];

    loop {
        stack.wait_link_up().await;
        link_led.set_high();
        defmt::info!("Network link is up");

        // Ensure DHCP configuration is up before trying connect
        stack.wait_config_up().await;
        defmt::info!("Network task initialized, config: {}", stack.config_v4());

        let mut udp = UdpSocket::new(
            stack,
            &mut rx_udp_meta,
            &mut rx_udp_buf,
            &mut tx_udp_meta,
            &mut tx_udp_buf,
        );
        defmt::info!("UDP socket bound to port {}", PORT);
        udp.bind(PORT).expect("failed to bind UDP socket");
        let mut remote_endpoint = None;
        loop {
            match select3(
                udp.recv_from(&mut rx_buffer),
                tm_rx.receive(),
                stack.wait_link_down(),
            )
            .await
            {
                Either3::First(Ok((len, meta))) => {
                    remote_endpoint = Some(meta.endpoint);
                    defmt::debug!("UDP RX {}, Meta: {}", len, meta);
                    tc_tx.send(rx_buffer[0..len].to_vec()).await;
                }
                Either3::First(Err(e)) => {
                    defmt::warn!("udp receive error: {}", e);
                    Timer::after_millis(100).await;
                }
                // TM is only generated as a response to a TC, so the endpoint is usually known.
                Either3::Second(packet) => match remote_endpoint {
                    Some(endpoint) => match udp.send_to(&packet, endpoint).await {
                        Ok(_) => defmt::debug!("UDP TX: {} bytes to: {}", packet.len(), endpoint),
                        Err(e) => defmt::warn!("udp send error: {}", e),
                    },
                    None => defmt::warn!("dropping TM, no remote endpoint known"),
                },
                Either3::Third(()) => {
                    defmt::warn!("Network link is down");
                    link_led.set_low();
                    break;
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn tc_handler(tc_rx: TcReceiver, tm_tx: TmSender) {
    let mut sequence_count = u14::new(0);
    loop {
        let tc = tc_rx.receive().await;
        defmt::info!("Received from UDP client: {}", tc.as_slice());
        let packet = match CcsdsPacketReader::new_with_checksum(&tc) {
            Ok(packet) => packet,
            Err(e) => {
                defmt::warn!("Failed to parse received TC packet: {}", e);
                continue;
            }
        };
        let tc_packet_id = CcsdsPacketIdAndPsc {
            packet_id: packet.packet_id(),
            psc: packet.psc(),
        };
        let Ok(request) = postcard::from_bytes::<stm32h7::Request>(packet.packet_data()) else {
            defmt::warn!("Failed to deserialize TC request");
            continue;
        };
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
                let freq_ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
                BLINK_FREQ_MS.store(freq_ms, Ordering::Relaxed);
                stm32h7::Response::Ok
            }
        };
        if let Err(e) = send_tm(tc_packet_id, response, sequence_count, &tm_tx).await {
            defmt::warn!("Failed to send TM response: {}", e);
        }
        sequence_count = sequence_count.wrapping_add(u14::new(1));
    }
}

async fn send_tm(
    tc_packet_id: CcsdsPacketIdAndPsc,
    response: stm32h7::Response,
    sequence_count: u14,
    sender: &TmSender,
) -> Result<(), CcsdsPacketCreationError> {
    let sp_header = SpHeader::new_for_unseg_tm(stm32h7::PUS_APID, sequence_count, 0);
    let tm_header = TmHeader {
        tc_packet_id: Some(tc_packet_id),
        uptime_millis: embassy_time::Instant::now().as_millis(),
    };
    let mut packet = alloc::vec![0; tm_size(&tm_header, &response)];
    create_tm_packet(&mut packet, sp_header, tm_header, response)?;
    sender.send(packet).await;
    Ok(())
}
