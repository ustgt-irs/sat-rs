#![no_std]
#![no_main]
use arbitrary_int::u14;
use cortex_m_semihosting::debug::{self, EXIT_FAILURE, EXIT_SUCCESS};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embedded_types::{create_tm_packet, stm32f3, tm_size, TmHeader};
use spacepackets::{CcsdsPacketCreationError, CcsdsPacketIdAndPsc, SpHeader};

use defmt_rtt as _; // global logger
use panic_probe as _;

use rtic::app;

const UART_BAUD: u32 = 115200;
const DEFAULT_BLINK_FREQ_MS: u32 = 1000;
const MAX_TC_LEN: usize = 128;
const MAX_TM_LEN: usize = 128;

// This is the predictable maximum overhead of the COBS encoding scheme.
// It is simply the maximum packet lenght dividied by 254 rounded up.
const COBS_TM_OVERHEAD: usize = cobs::max_encoding_overhead(MAX_TM_LEN);

const TM_BUF_LEN: usize = MAX_TC_LEN + COBS_TM_OVERHEAD;

const TC_DMA_BUF_LEN: usize = 512;

type TmPacket = heapless::Vec<u8, MAX_TM_LEN>;

static TM_QUEUE: embassy_sync::channel::Channel<CriticalSectionRawMutex, TmPacket, 16> =
    embassy_sync::channel::Channel::new();

#[derive(Debug, defmt::Format, thiserror::Error)]
pub enum TmSendError {
    #[error("packet creation error: {0}")]
    PacketCreation(#[from] CcsdsPacketCreationError),
    #[error("queue error")]
    Queue,
}

#[derive(Debug, defmt::Format)]
pub struct RequestWithTcId {
    pub request: stm32f3::Request,
    pub tc_id: CcsdsPacketIdAndPsc,
}

#[app(device = embassy_stm32)]
mod app {
    use core::time::Duration;

    use super::*;
    use arbitrary_int::u14;
    use embassy_time::Timer;
    use embedded_types::stm32f3::{Request, Response};
    use rtic::Mutex;
    use rtic_sync::{
        channel::{Receiver, Sender},
        make_channel,
    };
    use satrs_stm32f3_disco_rtic::LedPinSet;
    use spacepackets::CcsdsPacketReader;

    embassy_stm32::bind_interrupts!(struct Irqs {
        USART2 => embassy_stm32::usart::InterruptHandler<embassy_stm32::peripherals::USART2>;
        DMA1_CHANNEL6 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::DMA1_CH6>;
        DMA1_CHANNEL7 => embassy_stm32::dma::InterruptHandler<embassy_stm32::peripherals::DMA1_CH7>;
    });

    #[shared]
    struct Shared {
        blink_freq: Duration,
    }

    #[local]
    struct Local {
        leds: satrs_stm32f3_disco_rtic::Leds,
        current_dir: satrs_stm32f3_disco_rtic::Direction,
        seq_count: u14,
        tx: embassy_stm32::usart::UartTx<'static, embassy_stm32::mode::Async>,
        rx: embassy_stm32::usart::RingBufferedUartRx<'static>,
    }

    #[init]
    fn init(_cx: init::Context) -> (Shared, Local) {
        static DMA_BUF: static_cell::ConstStaticCell<[u8; TC_DMA_BUF_LEN]> =
            static_cell::ConstStaticCell::new([0; TC_DMA_BUF_LEN]);

        let p = embassy_stm32::init(Default::default());

        let (req_sender, req_receiver) = make_channel!(RequestWithTcId, 16);

        defmt::info!("sat-rs demo application for the STM32F3-Discovery with RTICv2");
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
        let leds = satrs_stm32f3_disco_rtic::Leds::new(led_pin_set);

        let mut config = embassy_stm32::usart::Config::default();
        config.baudrate = UART_BAUD;
        let uart = embassy_stm32::usart::Uart::new(
            p.USART2, p.PA3, p.PA2, p.DMA1_CH7, p.DMA1_CH6, Irqs, config,
        )
        .unwrap();

        let (tx, rx) = uart.split();
        defmt::info!("Spawning tasks");
        blinky::spawn().unwrap();
        serial_tx_handler::spawn().unwrap();
        serial_rx_handler::spawn(req_sender).unwrap();
        req_handler::spawn(req_receiver).unwrap();

        (
            Shared {
                blink_freq: Duration::from_millis(DEFAULT_BLINK_FREQ_MS as u64),
            },
            Local {
                leds,
                tx,
                seq_count: u14::new(0),
                rx: rx.into_ring_buffered(DMA_BUF.take()),
                current_dir: satrs_stm32f3_disco_rtic::Direction::North,
            },
        )
    }

    #[task(local = [leds, current_dir], shared=[blink_freq])]
    async fn blinky(mut cx: blinky::Context) {
        loop {
            cx.local.leds.blink_next(cx.local.current_dir);
            let current_blink_freq = cx.shared.blink_freq.lock(|current| *current);
            Timer::after_millis(current_blink_freq.as_millis() as u64).await;
        }
    }

    #[task(
        local = [
            tx,
            encoded_buf: [u8; TM_BUF_LEN] = [0; TM_BUF_LEN]
        ],
        shared = [],
    )]
    async fn serial_tx_handler(cx: serial_tx_handler::Context) {
        loop {
            loop {
                let vec = TM_QUEUE.receive().await;
                let encoded_len =
                    cobs::encode_including_sentinels(&vec[0..vec.len()], cx.local.encoded_buf);
                defmt::debug!("sending {} bytes over UART", encoded_len);
                cx.local
                    .tx
                    .write(&cx.local.encoded_buf[0..encoded_len])
                    .await
                    .unwrap();
            }
        }
    }

    #[task(
        local = [
            rx,
            read_buf: [u8; 128] = [0; 128],
            decode_buf: [u8; MAX_TC_LEN] = [0; MAX_TC_LEN],
        ],
        shared = [blink_freq]
    )]
    async fn serial_rx_handler(
        cx: serial_rx_handler::Context,
        mut sender: Sender<'static, RequestWithTcId, 16>,
    ) {
        let mut decoder = cobs::CobsDecoder::new(cx.local.decode_buf);
        loop {
            match cx.local.rx.read(cx.local.read_buf).await {
                Ok(bytes) => {
                    defmt::debug!("received {} bytes over UART", bytes);
                    for byte in cx.local.read_buf[0..bytes].iter() {
                        match decoder.feed(*byte) {
                            Ok(None) => (),
                            Ok(Some(packet_size)) => {
                                match CcsdsPacketReader::new_with_checksum(
                                    &decoder.dest()[0..packet_size],
                                ) {
                                    Ok(packet) => {
                                        let tc_packet_id =
                                            CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet);
                                        if let Ok(request) =
                                            postcard::from_bytes::<Request>(packet.packet_data())
                                        {
                                            sender
                                                .send(RequestWithTcId {
                                                    request,
                                                    tc_id: tc_packet_id,
                                                })
                                                .await
                                                .unwrap();
                                        }
                                    }
                                    Err(e) => {
                                        defmt::error!("error unpacking ccsds packet: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                defmt::error!("cobs decoding error: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    defmt::error!("uart read error: {}", e);
                }
            }
        }
    }

    #[task(shared = [blink_freq], local = [seq_count])]
    async fn req_handler(
        mut cx: req_handler::Context,
        mut receiver: Receiver<'static, RequestWithTcId, 16>,
    ) {
        loop {
            match receiver.recv().await {
                Ok(request_with_tc_id) => {
                    let tm_send_result = match request_with_tc_id.request {
                        Request::Ping => {
                            handle_ping_request(&mut cx, request_with_tc_id.tc_id).await
                        }
                        Request::ChangeBlinkFrequency(duration) => {
                            handle_change_blink_frequency_request(
                                &mut cx,
                                request_with_tc_id.tc_id,
                                duration,
                            )
                            .await
                        }
                    };
                    if let Err(e) = tm_send_result {
                        defmt::error!("error sending TM response: {}", e);
                    }
                }
                Err(_e) => defmt::error!("request receive error"),
            }
        }
    }

    async fn handle_ping_request(
        cx: &mut req_handler::Context<'_>,
        tc_packet_id: CcsdsPacketIdAndPsc,
    ) -> Result<(), TmSendError> {
        defmt::info!("Received PUS ping telecommand, sending ping reply");
        send_tm(tc_packet_id, Response::Ok, *cx.local.seq_count).await?;
        *cx.local.seq_count = cx.local.seq_count.wrapping_add(u14::new(1));
        Ok(())
    }

    async fn handle_change_blink_frequency_request(
        cx: &mut req_handler::Context<'_>,
        tc_packet_id: CcsdsPacketIdAndPsc,
        duration: Duration,
    ) -> Result<(), TmSendError> {
        defmt::info!(
            "Received ChangeBlinkFrequency request, new frequency: {} ms",
            duration.as_millis()
        );
        cx.shared
            .blink_freq
            .lock(|blink_freq| *blink_freq = duration);
        send_tm(tc_packet_id, Response::Ok, *cx.local.seq_count).await?;
        *cx.local.seq_count = cx.local.seq_count.wrapping_add(u14::new(1));
        Ok(())
    }
}

async fn send_tm(
    tc_packet_id: CcsdsPacketIdAndPsc,
    response: stm32f3::Response,
    current_seq_count: u14,
) -> Result<(), TmSendError> {
    let sp_header = SpHeader::new_for_unseg_tc(stm32f3::PUS_APID, current_seq_count, 0);
    let tm_header = TmHeader {
        tc_packet_id: Some(tc_packet_id),
        uptime_millis: embassy_time::Instant::now().as_millis(),
    };
    let mut tm_packet = TmPacket::new();
    let tm_size = tm_size(&tm_header, &response);
    tm_packet.resize(tm_size, 0).expect("vec resize failed");
    create_tm_packet(&mut tm_packet, sp_header, tm_header, response)?;
    TM_QUEUE.send(tm_packet).await;
    Ok(())
}

// same panicking *behavior* as `panic-probe` but doesn't print a panic message
// this prevents the panic message being printed *twice* when `defmt::panic` is invoked
#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf()
}

/// Terminates the application and makes a semihosting-capable debug tool exit
/// with status code 0.
pub fn exit() -> ! {
    loop {
        debug::exit(EXIT_SUCCESS);
    }
}

/// Hardfault handler.
///
/// Terminates the application and makes a semihosting-capable debug tool exit
/// with an error. This seems better than the default, which is to spin in a
/// loop.
#[cortex_m_rt::exception]
unsafe fn HardFault(_frame: &cortex_m_rt::ExceptionFrame) -> ! {
    loop {
        debug::exit(EXIT_FAILURE);
    }
}

// defmt-test 0.3.0 has the limitation that this `#[tests]` attribute can only be used
// once within a crate. the module can be in any file but there can only be at most
// one `#[tests]` module in this library crate
#[cfg(test)]
#[defmt_test::tests]
mod unit_tests {
    use defmt::assert;

    #[test]
    fn it_works() {
        assert!(true)
    }
}
