#![no_main]
#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use arbitrary_int::u14;
use defmt::Debug2Format;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_net::StackResources;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_stm32::{bind_interrupts, eth, gpio, peripherals, rng};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use spacepackets::{CcsdsPacketIdAndPsc, CcsdsPacketReader, SpHeader};
use static_cell::{ConstStaticCell, StaticCell};
use types::ccsds::{CcsdsCreationError, CcsdsTmPacketOwned};
use types::{Apid, ComponentId, Message, TcHeader, TmHeader, control, led, tmtc};

const HEARTBEAT_PERIOD: Duration = Duration::from_millis(500);
const DEFAULT_LED_MODE: led::Mode =
    led::Mode::AlternatingToggle(core::time::Duration::from_millis(1000));
const PORT: u16 = 7301;
const MTU: usize = 1500;

/// Locally administered MAC address
const MAC_ADDRESS: [u8; 6] = [0x02, 0x00, 0x11, 0x22, 0x33, 0x44];

const TC_QUEUE_DEPTH: usize = 32;
const TM_QUEUE_DEPTH: usize = 32;

static LED_MODE: Signal<CriticalSectionRawMutex, led::Mode> = Signal::new();

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

struct Leds {
    red: gpio::Output<'static>,
    orange: gpio::Output<'static>,
}

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
    let leds = Leds {
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
        udp_task(stack, tc_channel.sender(), tm_channel.receiver())
            .expect("spawning UDP task failed"),
    );
    spawner.spawn(heartbeat(green_led).expect("spawning heartbeat task failed"));
    spawner.spawn(led_task(leds).expect("spawning LED task failed"));
    spawner.spawn(
        tc_handler(
            tc_channel.receiver(),
            Telemetry {
                tx: tm_channel.sender(),
                sequence_count: u14::new(0),
            },
        )
        .expect("spawning TC handler task failed"),
    );
}

#[embassy_executor::task]
async fn heartbeat(mut led: gpio::Output<'static>) {
    loop {
        led.toggle();
        Timer::after(HEARTBEAT_PERIOD).await;
    }
}

/// Applies the current mode to the red and orange LED. A new mode is applied immediately.
#[embassy_executor::task]
async fn led_task(mut leds: Leds) {
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

#[embassy_executor::task]
async fn net_stack_task(mut runner: embassy_net::Runner<'static, Device>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn udp_task(stack: embassy_net::Stack<'static>, tc_tx: TcSender, tm_rx: TmReceiver) {
    // Task futures are allocated statically, so these buffers do not live on the stack.
    let mut rx_udp_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_udp_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_udp_buf = [0; MTU];
    let mut tx_udp_buf = [0; MTU];
    let mut rx_buffer = [0; MTU];

    loop {
        stack.wait_link_up().await;
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
        if let Err(e) = udp.bind(PORT) {
            defmt::error!("Failed to bind UDP socket: {}", e);
            Timer::after_secs(1).await;
            continue;
        }
        defmt::info!("UDP socket bound to port {}", PORT);
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
                    break;
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn tc_handler(tc_rx: TcReceiver, mut telemetry: Telemetry) {
    loop {
        let tc = tc_rx.receive().await;
        let packet = match CcsdsPacketReader::new_with_checksum(&tc) {
            Ok(packet) => packet,
            Err(e) => {
                defmt::warn!("Failed to parse received TC packet: {}", e);
                send_tmtc_event(&mut telemetry, tmtc::Event::InvalidTcPacket).await;
                continue;
            }
        };
        let tc_id = CcsdsPacketIdAndPsc {
            packet_id: packet.packet_id(),
            psc: packet.psc(),
        };
        let Ok((tc_header, payload)) = postcard::take_from_bytes::<TcHeader>(packet.user_data())
        else {
            defmt::warn!("Failed to deserialize TC header");
            send_tmtc_event(&mut telemetry, tmtc::Event::InvalidTcHeader).await;
            continue;
        };
        match tc_header.target_id {
            ComponentId::Controller => handle_controller_tc(payload, tc_id, &mut telemetry).await,
            ComponentId::Led => handle_led_tc(payload, tc_id, &mut telemetry).await,
            target_id => {
                defmt::warn!("No TC handler for target ID {}", Debug2Format(&target_id));
                send_tmtc_event(&mut telemetry, tmtc::Event::UnknownTargetId(target_id)).await;
            }
        }
    }
}

/// All TCs are received via UDP, so the UDP server is the sender of TMTC events.
async fn send_tmtc_event(telemetry: &mut Telemetry, event: tmtc::Event) {
    telemetry.send(ComponentId::UdpServer, None, &event).await;
}

/// The controller does not control anything yet, but handles generic requests like pings.
async fn handle_controller_tc(
    payload: &[u8],
    tc_id: CcsdsPacketIdAndPsc,
    telemetry: &mut Telemetry,
) {
    let Ok(request) = postcard::from_bytes::<control::request::Request>(payload) else {
        defmt::warn!("Failed to deserialize controller request");
        return;
    };
    match request {
        control::request::Request::Ping => defmt::info!("Received controller ping request"),
        control::request::Request::TestEvent => {
            defmt::info!("Received test event request");
            let event = types::Event::ControllerEvent(control::Event::TestEvent);
            telemetry.send(ComponentId::Controller, None, &event).await;
        }
    }
    telemetry
        .send(
            ComponentId::Controller,
            Some(tc_id),
            &control::response::Response::Ok,
        )
        .await;
}

async fn handle_led_tc(payload: &[u8], tc_id: CcsdsPacketIdAndPsc, telemetry: &mut Telemetry) {
    let Ok(request) = postcard::from_bytes::<led::request::Request>(payload) else {
        defmt::warn!("Failed to deserialize LED request");
        return;
    };
    match request {
        led::request::Request::Ping => defmt::info!("Received LED ping request"),
        led::request::Request::SetMode(mode) => {
            defmt::info!("Received LED mode request: {}", Debug2Format(&mode));
            LED_MODE.signal(mode);
        }
    }
    telemetry
        .send(ComponentId::Led, Some(tc_id), &led::response::Response::Ok)
        .await;
}

/// Packs TM and passes it to the UDP task.
struct Telemetry {
    tx: TmSender,
    sequence_count: u14,
}

impl Telemetry {
    /// TM without a TC ID is sent unsolicited, for example events.
    async fn send(
        &mut self,
        sender_id: ComponentId,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        payload: &(impl serde::Serialize + Message),
    ) {
        let sp_header = SpHeader::new_for_unseg_tm(Apid::Tmtc.raw_value(), self.sequence_count, 0);
        let tm_header = TmHeader::new_without_timestamp(
            sender_id,
            ComponentId::Ground,
            payload.message_type(),
            tc_id,
        );
        match CcsdsTmPacketOwned::new_with_serde_payload(sp_header, &tm_header, payload)
            .map_err(CcsdsCreationError::from)
            .and_then(|packet| packet.try_to_vec())
        {
            Ok(raw_packet) => {
                self.tx.send(raw_packet).await;
                self.sequence_count = self.sequence_count.wrapping_add(u14::new(1));
            }
            Err(e) => defmt::warn!("Failed to create TM packet: {}", Debug2Format(&e)),
        }
    }
}
